//! Server IP-to-name mapping.
//!
//! Maps RotMG game server IP addresses to human-readable names, including AWS IP ranges.

use std::net::Ipv4Addr;

/// Map a server IP address to a server name (like "EUWest", "USEast", etc).
///
/// Returns `None` if the IP is not a known RotMG server.
pub fn ip_to_server_name(ip: Ipv4Addr) -> Option<&'static str> {
    // Convert IP to big-endian u32
    let ip_int = u32::from_be_bytes(ip.octets()) as i32;

    match ip_int {
        // Base server mappings
        921430924 => Some("USWest4"),
        311434905 => Some("USWest3"),
        911617968 => Some("USWest"),
        916000068 => Some("USSouthWest"),
        886033951 => Some("USSouth3"),
        55737872 => Some("USSouth"),
        586068087 => Some("USNorthWest"),
        59571845 => Some("USMidWest2"),
        59051031 => Some("USMidWest"),
        316504123 => Some("USMidWest"), // 18.221.120.59
        878180357 => Some("USEast2"),
        921362968 => Some("USEast"),
        873486039 => Some("EUWest2"),
        267205855 => Some("EUWest"), // 15.237.60.223
        599016312 => Some("EUSouthWest"),
        312444280 => Some("EUNorth"),
        314104494 => Some("EUEast"),
        233592826 => Some("Australia"),
        57386221 => Some("Australia"), // 3.107.164.237
        50369407 => Some("Asia"),

        // Additional AWS IPs observed in the wild
        // EUWest (Paris region - eu-west-3)
        599058322 => Some("EUWest"), // 35.180.231.146
        220513064 => Some("EUWest"), // 13.36.195.40
        220588691 => Some("EUWest"), // 13.37.234.147

        // USEast2 (Virginia region - us-east-1)
        919705823 => Some("USEast2"), // 54.209.152.223

        // USEast (Virginia region - us-east-1)
        583796571 => Some("USEast"), // 34.204.7.91

        _ => None,
    }
}

/// Map a full server name (as returned by [`ip_to_server_name`]) to its short
/// community abbreviation (e.g. `"USMidWest2"` -> `"USMW2"`), used in clipboard
/// callouts. Unknown names fall back to the input unchanged.
pub fn short_server_name(name: &str) -> &str {
    match name {
        "USWest" => "USW",
        "USWest3" => "USW3",
        "USWest4" => "USW4",
        "USSouthWest" => "USSW",
        "USSouth" => "USS",
        "USSouth3" => "USS3",
        "USNorthWest" => "USNW",
        "USMidWest" => "USMW",
        "USMidWest2" => "USMW2",
        "USEast" => "USE",
        "USEast2" => "USE2",
        "EUWest" => "EUW",
        "EUWest2" => "EUW2",
        "EUSouthWest" => "EUSW",
        "EUNorth" => "EUN",
        "EUEast" => "EUE",
        "Australia" => "AUS",
        "Asia" => "ASIA",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_server_ips() {
        // EUWest at 15.237.60.223
        let ip = Ipv4Addr::new(15, 237, 60, 223);
        assert_eq!(ip_to_server_name(ip), Some("EUWest"));

        // Asia at 3.0.147.127
        let ip = Ipv4Addr::new(3, 0, 147, 127);
        assert_eq!(ip_to_server_name(ip), Some("Asia"));

        // Australia at 13.236.87.250
        let ip = Ipv4Addr::new(13, 236, 87, 250);
        assert_eq!(ip_to_server_name(ip), Some("Australia"));
    }

    #[test]
    fn unknown_ip_returns_none() {
        let ip = Ipv4Addr::new(192, 168, 1, 1);
        assert_eq!(ip_to_server_name(ip), None);
    }

    #[test]
    fn aws_euwest_variants() {
        // 35.180.231.146
        let ip = Ipv4Addr::new(35, 180, 231, 146);
        assert_eq!(ip_to_server_name(ip), Some("EUWest"));

        // 13.36.195.40
        let ip = Ipv4Addr::new(13, 36, 195, 40);
        assert_eq!(ip_to_server_name(ip), Some("EUWest"));

        // 13.37.234.147
        let ip = Ipv4Addr::new(13, 37, 234, 147);
        assert_eq!(ip_to_server_name(ip), Some("EUWest"));
    }

    #[test]
    fn aws_useast_variants() {
        // 54.209.152.223 is USEast2 (confirmed in the wild)
        let ip = Ipv4Addr::new(54, 209, 152, 223);
        assert_eq!(ip_to_server_name(ip), Some("USEast2"));

        // 34.204.7.91 is USEast
        let ip = Ipv4Addr::new(34, 204, 7, 91);
        assert_eq!(ip_to_server_name(ip), Some("USEast"));
    }

    #[test]
    fn short_server_names_abbreviate_known_servers() {
        assert_eq!(short_server_name("USMidWest2"), "USMW2");
        assert_eq!(short_server_name("USWest4"), "USW4");
        assert_eq!(short_server_name("EUWest"), "EUW");
        assert_eq!(short_server_name("Australia"), "AUS");
        assert_eq!(short_server_name("Asia"), "ASIA");
        // Unknown names pass through unchanged.
        assert_eq!(short_server_name("MysteryRealm"), "MysteryRealm");
    }
}
