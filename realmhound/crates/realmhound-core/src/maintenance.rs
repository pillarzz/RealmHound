//! In-memory lifecycle logging for resumable history maintenance.

use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct MaintenanceLog {
    jobs: HashMap<&'static str, JobState>,
    current: Option<&'static str>,
    attempted: Vec<&'static str>,
    completed: Vec<(&'static str, i64)>,
}

struct JobState {
    since: Instant,
    started: bool,
    deferred: Option<&'static str>,
    failure: Option<Failure>,
}

impl Default for JobState {
    fn default() -> Self {
        Self {
            since: Instant::now(),
            started: false,
            deferred: None,
            failure: None,
        }
    }
}

struct Failure {
    message: String,
    last_warning: Instant,
    attempts: u64,
    retry_announced: bool,
}

impl MaintenanceLog {
    pub fn begin_batch(&mut self, job: &'static str) {
        self.current = None;
        self.attempted.clear();
        self.completed.clear();
        self.task(job);
    }

    /// Set error context without announcing an ongoing service on every poll.
    pub fn task(&mut self, job: &'static str) {
        self.current = Some(job);
        if !self.attempted.contains(&job) {
            self.attempted.push(job);
        }
        if let Some(failure) = self
            .jobs
            .get_mut(job)
            .and_then(|state| state.failure.as_mut())
        {
            if !failure.retry_announced {
                tracing::info!(job, "[MAINTENANCE] Retrying");
                failure.retry_announced = true;
            }
        }
    }

    pub fn start(&mut self, job: &'static str, cursor: i64, cutoff: i64, resumed: bool) {
        self.task(job);
        let state = self.jobs.entry(job).or_default();
        if !state.started {
            let action = if resumed || state.deferred.is_some() {
                "Resumed"
            } else {
                "Started"
            };
            tracing::info!(job, cursor, cutoff, "[MAINTENANCE] {action}");
            state.started = true;
        } else if state.deferred.is_some() {
            tracing::info!(job, cursor, cutoff, "[MAINTENANCE] Resumed after deferral");
        }
        state.deferred = None;
    }

    pub fn deferred(&mut self, job: &'static str, reason: &'static str) {
        self.current = Some(job);
        let state = self.jobs.entry(job).or_default();
        if state.deferred != Some(reason) {
            tracing::info!(job, reason, "[MAINTENANCE] Deferred");
            state.deferred = Some(reason);
        }
    }

    /// Completion is emitted only if the enclosing transaction succeeds.
    pub fn complete(&mut self, job: &'static str, cursor: i64) {
        self.completed.push((job, cursor));
    }

    pub fn finish<T>(&mut self, result: &rusqlite::Result<T>) {
        if let Err(error) = result {
            self.failed(&error.to_string());
            self.completed.clear();
            return;
        }
        for job in &self.attempted {
            if let Some(failure) = self
                .jobs
                .get_mut(job)
                .and_then(|state| state.failure.take())
            {
                tracing::info!(
                    job,
                    failed_attempts = failure.attempts,
                    "[MAINTENANCE] Recovered after retry"
                );
            }
        }
        for (job, cursor) in self.completed.drain(..) {
            if let Some(state) = self.jobs.remove(job) {
                tracing::info!(
                    job,
                    cursor,
                    elapsed_ms = state.since.elapsed().as_millis() as u64,
                    "[MAINTENANCE] Completed"
                );
            }
        }
    }

    pub fn failed(&mut self, message: &str) {
        let job = self.current.unwrap_or("history-maintenance");
        let state = self.jobs.entry(job).or_default();
        let now = Instant::now();
        match state.failure.as_mut() {
            Some(failure) if failure.message == message => {
                failure.attempts += 1;
                if now.duration_since(failure.last_warning) < Duration::from_secs(60) {
                    return;
                }
                failure.last_warning = now;
            }
            _ => {
                state.failure = Some(Failure {
                    message: message.to_string(),
                    last_warning: now,
                    attempts: 1,
                    retry_announced: false,
                });
            }
        }
        tracing::warn!(
            job,
            error = message,
            retry_delay_secs = 5,
            "[MAINTENANCE] Failed; scheduled for retry"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_waits_for_commit_and_survives_retry() {
        let mut log = MaintenanceLog::default();
        log.begin_batch("test-job");
        log.start("test-job", 64, 70, true);
        log.complete("test-job", 70);
        log.finish(&Err::<(), _>(rusqlite::Error::InvalidQuery));
        assert!(log.jobs["test-job"].failure.is_some());
        assert!(log.completed.is_empty());

        log.begin_batch("test-job");
        log.start("test-job", 64, 70, true);
        assert!(
            log.jobs["test-job"]
                .failure
                .as_ref()
                .unwrap()
                .retry_announced
        );
        log.complete("test-job", 70);
        log.finish(&Ok(()));
        assert!(!log.jobs.contains_key("test-job"));
    }

    #[test]
    fn deferrals_and_repeated_failures_keep_their_state() {
        let mut log = MaintenanceLog::default();
        log.deferred("test-job", "waiting for assets");
        let since = log.jobs["test-job"].since;
        log.deferred("test-job", "waiting for assets");
        assert_eq!(log.jobs["test-job"].since, since);
        log.start("test-job", 0, 100, false);
        assert!(log.jobs["test-job"].deferred.is_none());
        log.failed("busy");
        let warning = log.jobs["test-job"].failure.as_ref().unwrap().last_warning;
        log.failed("busy");
        let failure = log.jobs["test-job"].failure.as_ref().unwrap();
        assert_eq!(failure.attempts, 2);
        assert_eq!(failure.last_warning, warning);
        log.jobs
            .get_mut("test-job")
            .unwrap()
            .failure
            .as_mut()
            .unwrap()
            .last_warning = Instant::now() - Duration::from_secs(61);
        log.failed("busy");
        assert!(log.jobs["test-job"].failure.as_ref().unwrap().last_warning > warning);
        log.failed("different failure");
        assert_eq!(log.jobs["test-job"].failure.as_ref().unwrap().attempts, 1);
        log.finish(&Ok(()));
        assert!(log.jobs["test-job"].failure.is_none());
    }

    #[test]
    fn idle_services_do_not_allocate_job_states() {
        let mut log = MaintenanceLog::default();
        for _ in 0..10 {
            log.begin_batch("combat-maintenance");
            log.task("death-reconciliation");
            log.finish(&Ok(()));
        }
        assert!(log.jobs.is_empty());
    }
}
