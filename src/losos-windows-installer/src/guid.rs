//! GUIDs as GPT and UEFI store them.
//!
//! A GUID is written as text with its first three groups big-endian, and
//! stored on disk, in a UEFI device path and in Windows' `GUID` struct with
//! those three groups little-endian. Getting that wrong builds and runs, and
//! names a partition that does not exist. [`Guid`] holds the text order, and
//! `to_bytes_le`/`from_bytes_le` (or `as_fields`/`from_fields` for Windows'
//! struct) are the only ways to and from the stored one.
pub use uuid::Uuid as Guid;

/// Partition types, as `systemd-id128 show` lists them.
pub mod types {
    use super::Guid;
    use uuid::uuid;

    pub const ESP: Guid = uuid!("c12a7328-f81f-11d2-ba4b-00a0c93ec93b");
    /// The Boot Loader Specification's extended boot loader partition, where
    /// the UKIs go when the ESP is Windows' own and too small for two.
    pub const XBOOTLDR: Guid = uuid!("bc13c2ff-59e6-4262-a352-b275fd6f7172");
    /// What Windows formats C: as.
    pub const MICROSOFT_BASIC_DATA: Guid = uuid!("ebd0a0a2-b9e5-4433-87c0-68b6b72699c7");
    pub const HOME: Guid = uuid!("933ac7e1-2eb4-4f13-b844-0e14e2aef915");
    pub const SWAP: Guid = uuid!("0657fd6d-a4ab-43c4-84e5-0933c84b4f4f");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_text() {
        let text = "8484680c-9521-48c6-9c11-b0720656f69e";
        assert_eq!(Guid::parse_str(text).unwrap().to_string(), text);
        assert_eq!(
            Guid::parse_str("{8484680C-9521-48C6-9C11-B0720656F69E}")
                .unwrap()
                .to_string(),
            text
        );
        assert!(Guid::parse_str("8484680c-9521-48c6-9c11").is_err());
        assert!(Guid::parse_str("8484680c-9521-48c6-9c11-b0720656f69g").is_err());
    }

    #[test]
    fn stores_the_first_three_groups_little_endian() {
        // The ESP's type as it appears in a GPT entry on disk.
        let stored = types::ESP.to_bytes_le();
        assert_eq!(
            stored,
            [
                0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e,
                0xc9, 0x3b
            ]
        );
        assert_eq!(Guid::from_bytes_le(stored), types::ESP);
    }

    #[test]
    fn fields_match_windows_guid() {
        let (d1, d2, d3, d4) = types::ESP.as_fields();
        assert_eq!((d1, d2, d3), (0xc12a7328, 0xf81f, 0x11d2));
        assert_eq!(d4, &[0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b]);
        assert_eq!(Guid::from_fields(d1, d2, d3, d4), types::ESP);
    }
}
