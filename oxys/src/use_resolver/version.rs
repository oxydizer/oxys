use std::cmp::Ordering;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ParsedVersion {
    numbers: Vec<u64>,
    letter_suffix: String,
    suffixes: Vec<VersionSuffix>,
    revision: u64,
}

impl ParsedVersion {
    pub fn parse(value: &str) -> Self {
        let (without_revision, revision) = parse_revision(value);
        let mut parts = without_revision.split('_');
        let main = parts.next().unwrap_or_default();
        let (numbers, letter_suffix) = parse_main_version(main);
        let suffixes = parts.filter_map(VersionSuffix::parse).collect();

        Self {
            numbers,
            letter_suffix,
            suffixes,
            revision,
        }
    }
}

impl Ord for ParsedVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_number_lists(&self.numbers, &other.numbers)
            .then_with(|| self.letter_suffix.cmp(&other.letter_suffix))
            .then_with(|| compare_suffix_lists(&self.suffixes, &other.suffixes))
            .then_with(|| self.revision.cmp(&other.revision))
    }
}

impl PartialOrd for ParsedVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct VersionSuffix {
    kind: SuffixKind,
    number: u64,
}

impl VersionSuffix {
    fn parse(value: &str) -> Option<Self> {
        let split_at = value
            .char_indices()
            .find(|(_, ch)| ch.is_ascii_digit())
            .map(|(idx, _)| idx)
            .unwrap_or(value.len());
        let kind = SuffixKind::parse(&value[..split_at])?;
        let number = value
            .get(split_at..)
            .filter(|suffix| !suffix.is_empty())
            .and_then(|suffix| suffix.parse::<u64>().ok())
            .unwrap_or(0);

        Some(Self { kind, number })
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
enum SuffixKind {
    Alpha,
    Beta,
    Pre,
    Rc,
    Patch,
}

impl SuffixKind {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "alpha" => Some(Self::Alpha),
            "beta" => Some(Self::Beta),
            "pre" => Some(Self::Pre),
            "rc" => Some(Self::Rc),
            "p" => Some(Self::Patch),
            _ => None,
        }
    }

    fn rank(self) -> i8 {
        match self {
            Self::Alpha => -4,
            Self::Beta => -3,
            Self::Pre => -2,
            Self::Rc => -1,
            Self::Patch => 1,
        }
    }
}

pub fn normalize_version(version: Option<String>) -> Option<String> {
    match version {
        Some(version) if version.trim().is_empty() => None,
        Some(version) => Some(version),
        None => None,
    }
}

pub fn compare_gentoo_versions(left: &str, right: &str) -> Ordering {
    let left = ParsedVersion::parse(left);
    let right = ParsedVersion::parse(right);

    left.cmp(&right)
}

pub fn is_live_version(version: &str) -> bool {
    let (base, _) = parse_revision(version);
    let numeric_base = base.split('_').next().unwrap_or(base);

    numeric_base == "9999" || numeric_base.ends_with(".9999")
}

pub fn parse_revision(value: &str) -> (&str, u64) {
    if let Some((base, revision)) = value.rsplit_once("-r")
        && let Ok(revision) = revision.parse::<u64>()
    {
        return (base, revision);
    }

    (value, 0)
}

fn parse_main_version(value: &str) -> (Vec<u64>, String) {
    let split_at = value
        .char_indices()
        .find(|(_, ch)| ch.is_ascii_alphabetic())
        .map(|(idx, _)| idx)
        .unwrap_or(value.len());
    let numeric = &value[..split_at];
    let letter_suffix = value[split_at..].to_owned();
    let numbers = numeric
        .split('.')
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect::<Vec<_>>();

    (numbers, letter_suffix)
}

fn compare_number_lists(left: &[u64], right: &[u64]) -> Ordering {
    let max_len = left.len().max(right.len());

    for index in 0..max_len {
        let left_number = left.get(index).copied().unwrap_or(0);
        let right_number = right.get(index).copied().unwrap_or(0);
        let ordering = left_number.cmp(&right_number);
        if ordering != Ordering::Equal {
            return ordering;
        }
    }

    Ordering::Equal
}

fn compare_suffix_lists(left: &[VersionSuffix], right: &[VersionSuffix]) -> Ordering {
    let max_len = left.len().max(right.len());

    for index in 0..max_len {
        let left_rank = left
            .get(index)
            .map(|suffix| suffix.kind.rank())
            .unwrap_or(0);
        let right_rank = right
            .get(index)
            .map(|suffix| suffix.kind.rank())
            .unwrap_or(0);
        let ordering = left_rank.cmp(&right_rank);
        if ordering != Ordering::Equal {
            return ordering;
        }

        let left_number = left.get(index).map(|suffix| suffix.number).unwrap_or(0);
        let right_number = right.get(index).map(|suffix| suffix.number).unwrap_or(0);
        let ordering = left_number.cmp(&right_number);
        if ordering != Ordering::Equal {
            return ordering;
        }
    }

    Ordering::Equal
}
