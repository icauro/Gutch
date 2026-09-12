use serde::{Deserialize, Serialize};

/// Optional, conservative filename portability requirements for every snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepositoryConstraints {
    #[serde(default)]
    pub targets: Vec<FilesystemTarget>,
}

impl PartialEq for RepositoryConstraints {
    fn eq(&self, other: &Self) -> bool {
        self.targets
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            == other
                .targets
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
    }
}

#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
pub enum FilesystemTarget {
    WindowsNtfs,
    WindowsExfat,
    LinuxExt4,
    LinuxExt4Casefold,
    LinuxBtrfs,
    MacosApfs,
    MacosApfsCaseSensitive,
    IosApfs,
    AndroidExt4,
    AndroidF2fs,
}

impl FilesystemTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WindowsNtfs => "windows-ntfs",
            Self::WindowsExfat => "windows-exfat",
            Self::LinuxExt4 => "linux-ext4",
            Self::LinuxExt4Casefold => "linux-ext4-casefold",
            Self::LinuxBtrfs => "linux-btrfs",
            Self::MacosApfs => "macos-apfs",
            Self::MacosApfsCaseSensitive => "macos-apfs-case-sensitive",
            Self::IosApfs => "ios-apfs",
            Self::AndroidExt4 => "android-ext4",
            Self::AndroidF2fs => "android-f2fs",
        }
    }
}
