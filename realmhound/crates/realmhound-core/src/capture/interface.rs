//! Network interface discovery and management.

use super::error::CaptureError;

/// Represents a network interface available for packet capture.
#[derive(Debug, Clone)]
pub struct NetworkInterface {
    /// System name of the interface (e.g., "\\Device\\NPF_{GUID}")
    pub name: String,
    /// Human-readable description
    pub description: Option<String>,
    /// IPv4 addresses assigned to this interface
    pub addresses: Vec<std::net::Ipv4Addr>,
}

impl NetworkInterface {
    /// List all available network interfaces.
    ///
    /// # Errors
    ///
    /// Returns an error if pcap fails to enumerate interfaces or if
    /// Npcap is not installed.
    pub fn list_all() -> Result<Vec<NetworkInterface>, CaptureError> {
        let devices = pcap::Device::list().map_err(|e| {
            if e.to_string().contains("not have permission")
                || e.to_string().contains("No such file")
            {
                CaptureError::NpcapNotInstalled
            } else {
                CaptureError::PcapError(e)
            }
        })?;

        if devices.is_empty() {
            return Err(CaptureError::NoInterfaces);
        }

        let interfaces = devices
            .into_iter()
            .map(|dev| {
                let addresses = dev
                    .addresses
                    .iter()
                    .filter_map(|addr| {
                        if let std::net::IpAddr::V4(ipv4) = addr.addr {
                            Some(ipv4)
                        } else {
                            None
                        }
                    })
                    .collect();

                NetworkInterface {
                    name: dev.name,
                    description: dev.desc,
                    addresses,
                }
            })
            .collect();

        Ok(interfaces)
    }

    /// Get a display name for this interface.
    pub fn display_name(&self) -> &str {
        self.description.as_deref().unwrap_or(&self.name)
    }

    /// Check if this interface has any IPv4 addresses.
    pub fn has_ipv4_address(&self) -> bool {
        !self.addresses.is_empty()
    }

    /// Check if this interface looks like a real network adapter.
    ///
    /// Filters out loopback and link-local addresses.
    pub fn is_likely_active(&self) -> bool {
        self.addresses
            .iter()
            .any(|addr| !addr.is_loopback() && !addr.is_link_local())
    }

    /// Find the best interface for capturing game traffic.
    ///
    /// Prioritizes interfaces with non-loopback IPv4 addresses.
    /// Returns `None` if no suitable interface is found.
    pub fn find_best() -> Result<Option<NetworkInterface>, CaptureError> {
        let interfaces = Self::list_all()?;

        // First, try to find an interface with a real IPv4 address
        if let Some(iface) = interfaces.iter().find(|i| i.is_likely_active()) {
            return Ok(Some(iface.clone()));
        }

        // Fall back to any interface with an IPv4 address
        if let Some(iface) = interfaces.iter().find(|i| i.has_ipv4_address()) {
            return Ok(Some(iface.clone()));
        }

        // Return the first interface if nothing else matches
        Ok(interfaces.into_iter().next())
    }
}
