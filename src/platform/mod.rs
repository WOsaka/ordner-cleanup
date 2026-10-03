pub mod windows;

/// Windows-Dateiattribute (`dwFileAttributes`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FileAttrs(pub u32);

impl FileAttrs {
    pub const READONLY: u32 = 0x1;
    pub const HIDDEN: u32 = 0x2;
    pub const SYSTEM: u32 = 0x4;
    pub const DIRECTORY: u32 = 0x10;
    pub const REPARSE_POINT: u32 = 0x400;
    pub const OFFLINE: u32 = 0x1000;
    pub const RECALL_ON_OPEN: u32 = 0x4_0000;
    pub const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;

    pub fn is_hidden(self) -> bool {
        self.0 & Self::HIDDEN != 0
    }
    pub fn is_system(self) -> bool {
        self.0 & Self::SYSTEM != 0
    }
    /// OneDrive-Platzhalter: Inhalt nur in der Cloud, Öffnen würde einen Download auslösen.
    pub fn is_cloud_only(self) -> bool {
        self.0 & (Self::RECALL_ON_DATA_ACCESS | Self::RECALL_ON_OPEN | Self::OFFLINE) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_only_bei_jedem_der_drei_flags() {
        for flag in [
            FileAttrs::RECALL_ON_DATA_ACCESS,
            FileAttrs::RECALL_ON_OPEN,
            FileAttrs::OFFLINE,
        ] {
            assert!(FileAttrs(flag).is_cloud_only(), "{flag:#x}");
            assert!(FileAttrs(flag | FileAttrs::READONLY).is_cloud_only());
        }
    }

    #[test]
    fn normale_datei_ist_nicht_cloud_only() {
        assert!(!FileAttrs(0x20).is_cloud_only()); // ARCHIVE
        assert!(!FileAttrs(FileAttrs::REPARSE_POINT).is_cloud_only());
    }

    #[test]
    fn hidden_und_system() {
        let a = FileAttrs(FileAttrs::HIDDEN | FileAttrs::SYSTEM);
        assert!(a.is_hidden() && a.is_system());
        assert!(!FileAttrs(0x20).is_hidden());
        assert!(!FileAttrs(0x20).is_system());
    }
}
