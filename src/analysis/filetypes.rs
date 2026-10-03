#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Dokumente,
    Bilder,
    Video,
    Audio,
    Archive,
    Code,
    Ausfuehrbar,
    Sonstige,
}

impl Category {
    pub const ALL: [Category; 8] = [
        Self::Dokumente,
        Self::Bilder,
        Self::Video,
        Self::Audio,
        Self::Archive,
        Self::Code,
        Self::Ausfuehrbar,
        Self::Sonstige,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Dokumente => "Dokumente",
            Self::Bilder => "Bilder",
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Archive => "Archive",
            Self::Code => "Code",
            Self::Ausfuehrbar => "Ausführbar",
            Self::Sonstige => "Sonstige",
        }
    }
}

/// Kategorie anhand der Endung (ohne Punkt, Groß-/Kleinschreibung egal).
pub fn categorize(ext: Option<&str>) -> Category {
    let Some(ext) = ext else {
        return Category::Sonstige;
    };
    match ext.to_lowercase().as_str() {
        "doc" | "docx" | "dot" | "dotx" | "odt" | "rtf" | "txt" | "md" | "pdf" | "xls" | "xlsx"
        | "xlsm" | "ods" | "csv" | "ppt" | "pptx" | "odp" | "msg" | "eml" | "one" | "vsd"
        | "vsdx" => Category::Dokumente,
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "tif" | "tiff" | "webp" | "heic" | "svg"
        | "ico" | "raw" | "cr2" | "nef" | "psd" => Category::Bilder,
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" | "m4v" | "mpg" | "mpeg" => {
            Category::Video
        }
        "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" | "wma" | "opus" => Category::Audio,
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "iso" | "cab" | "tgz" => {
            Category::Archive
        }
        "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "java" | "c" | "h" | "cpp" | "hpp" | "cs"
        | "go" | "rb" | "php" | "sql" | "sh" | "ps1" | "bat" | "cmd" | "html" | "htm" | "css"
        | "json" | "xml" | "yaml" | "yml" | "toml" | "sln" | "csproj" => Category::Code,
        "exe" | "msi" | "dll" | "sys" | "com" | "scr" | "msix" | "appx" => Category::Ausfuehrbar,
        _ => Category::Sonstige,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(Some("docx"), Category::Dokumente)]
    #[case(Some("PDF"), Category::Dokumente)]
    #[case(Some("jpg"), Category::Bilder)]
    #[case(Some("mp4"), Category::Video)]
    #[case(Some("flac"), Category::Audio)]
    #[case(Some("7z"), Category::Archive)]
    #[case(Some("rs"), Category::Code)]
    #[case(Some("exe"), Category::Ausfuehrbar)]
    #[case(Some("xyz123"), Category::Sonstige)]
    #[case(None, Category::Sonstige)]
    fn kategorien(#[case] ext: Option<&str>, #[case] expected: Category) {
        assert_eq!(categorize(ext), expected);
    }

    #[test]
    fn acht_kategorien_mit_eindeutigen_labels() {
        let labels: std::collections::HashSet<_> =
            Category::ALL.iter().map(|c| c.label()).collect();
        assert_eq!(labels.len(), 8);
    }
}
