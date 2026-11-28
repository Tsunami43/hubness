//! Semantic versions of hubness releases (`hubness update`, `GET /api/system/version`).

use std::cmp::Ordering;

/// A semantic version: `1.2.3` or `1.2.3-rc.1` (build metadata ignored).
#[derive(Debug, PartialEq, Eq)]
pub struct Version {
    pub core: [u64; 3],
    pub pre: Vec<String>,
}

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('v');
        let s = s.split('+').next()?;
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, p.split('.').map(String::from).collect()),
            None => (s, vec![]),
        };
        let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
        let v = [parts.next()??, parts.next()??, parts.next()??];
        if parts.next().is_some() {
            return None;
        }
        Some(Self { core: v, pre })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core.cmp(&other.core).then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => {
                for (a, b) in self.pre.iter().zip(&other.pre) {
                    let o = match (a.parse::<u64>(), b.parse::<u64>()) {
                        (Ok(x), Ok(y)) => x.cmp(&y),
                        (Ok(_), Err(_)) => Ordering::Less,
                        (Err(_), Ok(_)) => Ordering::Greater,
                        _ => a.cmp(b),
                    };
                    if o != Ordering::Equal {
                        return o;
                    }
                }
                self.pre.len().cmp(&other.pre.len())
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// `a` against `b`; an unparsable version counts as the oldest.
pub fn compare(a: &str, b: &str) -> Ordering {
    match (Version::parse(a), Version::parse(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_like_semver() {
        assert_eq!(compare("0.2.0", "0.1.9"), Ordering::Greater);
        assert_eq!(compare("0.10.0", "0.9.0"), Ordering::Greater);
        assert_eq!(compare("v1.0.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare("1.0.0-rc.1", "1.0.0"), Ordering::Less);
        assert_eq!(compare("1.0.0-rc.2", "1.0.0-rc.10"), Ordering::Less);
        assert_eq!(compare("1.0.0-alpha", "1.0.0-alpha.1"), Ordering::Less);
        assert_eq!(compare("1.0.0-beta", "1.0.0-alpha.1"), Ordering::Greater);
        assert!(Version::parse("1.2").is_none());
        assert!(Version::parse("1.2.3.4").is_none());
        assert!(Version::parse("latest").is_none());
    }
}
