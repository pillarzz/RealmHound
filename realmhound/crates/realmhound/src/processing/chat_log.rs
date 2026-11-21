//! Worker-side per-account daily chat log writer.
//!
//! Chat persistence runs on the worker so a stalled or hidden UI cannot lose
//! lines (I2). The daily filename uses the message timestamp, not the write
//! time, so a line delivered after midnight stays on the day it was sent.

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use chrono::NaiveDate;

use crate::panels::chat::{ChatMessage, ChatType};

/// Append-only writer that keeps one daily file open at a time.
pub(crate) struct ChatLogWriter {
    dir: PathBuf,
    current: Option<(NaiveDate, BufWriter<File>)>,
}

impl ChatLogWriter {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self { dir, current: None }
    }

    /// Append one message. Returns an IO error so the worker can surface it in
    /// health; non-persisted chat types are ignored.
    pub(crate) fn append(&mut self, message: &ChatMessage) -> std::io::Result<()> {
        let Some(line) = format_line(message) else {
            return Ok(());
        };
        if self.dir.as_os_str().is_empty() {
            return Ok(());
        }

        let date = message.timestamp.date_naive();
        let writer = match self.current.take() {
            Some((current_date, writer)) if current_date == date => writer,
            _ => {
                fs::create_dir_all(&self.dir)?;
                let path = self
                    .dir
                    .join(format!("chat_{}.log", date.format("%Y-%m-%d")));
                BufWriter::new(OpenOptions::new().create(true).append(true).open(path)?)
            }
        };
        let mut writer = writer;
        let result = writer
            .write_all(line.as_bytes())
            .and_then(|()| writer.flush());
        self.current = Some((date, writer));
        result
    }
}

fn format_line(message: &ChatMessage) -> Option<String> {
    let type_str = match message.chat_type {
        ChatType::Normal => "[PUBLIC]",
        ChatType::Guild => "[GUILD]",
        ChatType::Party => "[PARTY]",
        ChatType::Whisper => "[WHISPER]",
        ChatType::Announcement | ChatType::Enemy => return None,
    };
    let time_str = message.timestamp.format("%H:%M:%S");
    let mut meta = String::new();
    if message.stars > 0 {
        meta.push_str(&format!("★{} ", message.stars));
    }
    if message.is_supporter {
        meta.push_str("♦ ");
    }
    Some(match &message.recipient {
        Some(recipient) => format!(
            "{} {} {}{} → {}: {}\n",
            time_str, type_str, meta, message.sender, recipient, message.text
        ),
        None => format!(
            "{} {} {}{}: {}\n",
            time_str, type_str, meta, message.sender, message.text
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};

    fn message(at: chrono::DateTime<Local>, chat_type: ChatType) -> ChatMessage {
        ChatMessage {
            timestamp: at,
            sender: "Pillar".to_string(),
            text: "hello".to_string(),
            chat_type,
            recipient: None,
            stars: 0,
            is_supporter: false,
            boss_id: None,
            is_outgoing: false,
        }
    }

    #[test]
    fn writes_to_the_message_timestamp_day() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("logs").join("chat");
        let mut writer = ChatLogWriter::new(dir.clone());

        writer
            .append(&message(
                Local.with_ymd_and_hms(2026, 1, 1, 23, 59, 0).unwrap(),
                ChatType::Normal,
            ))
            .unwrap();
        writer
            .append(&message(
                Local.with_ymd_and_hms(2026, 1, 2, 0, 1, 0).unwrap(),
                ChatType::Guild,
            ))
            .unwrap();

        let day_one = std::fs::read_to_string(dir.join("chat_2026-01-01.log")).unwrap();
        let day_two = std::fs::read_to_string(dir.join("chat_2026-01-02.log")).unwrap();
        assert!(day_one.contains("23:59:00 [PUBLIC] Pillar: hello"));
        assert!(day_two.contains("00:01:00 [GUILD] Pillar: hello"));
    }

    #[test]
    fn ignores_non_persisted_chat_types() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("chat");
        let mut writer = ChatLogWriter::new(dir.clone());

        writer
            .append(&message(Local::now(), ChatType::Announcement))
            .unwrap();
        assert!(!dir.exists(), "no daily file for a non-persisted type");
    }
}
