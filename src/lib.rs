use core::{fmt, str::FromStr};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    major: u16,
    minor: u16,
    patch: u16,
}
impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}
impl fmt::Debug for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <Self as fmt::Display>::fmt(self, f)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct FullVersion {
    version: Version,
    prerelease: Option<(String, u16)>,
}
impl FromStr for FullVersion {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut iter = s.splitn(3, '.');
        let major = iter.next().ok_or(())?.parse().map_err(|_| ())?;
        let minor = iter.next().ok_or(())?.parse().map_err(|_| ())?;
        let s = iter.next().ok_or(())?;
        let (patch, prerelease) = match s.split_once('-') {
            Some((patch, prerelease)) => {
                let (stream, version) = prerelease.split_once('.').ok_or(())?;
                let patch = patch.parse().map_err(|_| ())?;
                let version = version.parse().map_err(|_| ())?;
                (patch, Some((stream.into(), version)))
            }
            None => (s.parse().map_err(|_| ())?, None),
        };
        Ok(Self {
            version: Version {
                major,
                minor,
                patch,
            },
            prerelease,
        })
    }
}
impl fmt::Display for FullVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.version.fmt(f)?;
        if let Some((name, version)) = &self.prerelease {
            write!(f, "-{}.{}", name, version)?;
        }
        Ok(())
    }
}
impl fmt::Debug for FullVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <Self as fmt::Display>::fmt(self, f)
    }
}

/// Stores the latest stable version, as well as the latest prerelease version if it's greater than the latest stable version.
#[derive(Default)]
pub struct LatestVersions {
    stable: Option<Version>,
    prerelease: Option<Version>,
    // There's no way to tell which prerelease stream is greater than the other, so store the latest version for each.
    prereleases: Vec<(String, u16)>,
}
impl LatestVersions {
    pub fn push(&mut self, version: FullVersion) {
        match version.prerelease {
            Some((stream, prerelease)) => {
                if self.stable.map_or(true, |stable| version.version > stable) {
                    if self
                        .prerelease
                        .map_or(true, |prerelease| version.version > prerelease)
                    {
                        self.prerelease = Some(version.version);
                        self.prereleases.clear();
                        self.prereleases.push((stream, prerelease));
                    } else if self
                        .prerelease
                        .map_or(false, |prerelease| version.version == prerelease)
                    {
                        if let Some((_, v)) =
                            self.prereleases.iter_mut().find(|(s, _)| stream == *s)
                        {
                            *v = prerelease.max(*v);
                        } else {
                            self.prereleases.push((stream, prerelease));
                        }
                    }
                }
            }
            None => {
                if self.stable.map_or(true, |stable| version.version > stable) {
                    self.stable = Some(version.version);
                    if self
                        .prerelease
                        .map_or(false, |prerelease| version.version >= prerelease)
                    {
                        self.prerelease = None;
                        self.prereleases.clear();
                    }
                }
            }
        }
    }

    pub fn versions(&self) -> impl Iterator<Item = FullVersion> + '_ {
        self.stable
            .into_iter()
            .map(|version| FullVersion {
                version,
                prerelease: None,
            })
            .chain(self.prerelease.into_iter().flat_map(|version| {
                self.prereleases.iter().map(move |prerelease| FullVersion {
                    version,
                    prerelease: Some(prerelease.clone()),
                })
            }))
    }
}

pub struct CrateName<'a> {
    pub name: &'a str,
    pub version: FullVersion,
}
impl<'a> CrateName<'a> {
    pub fn from_file_name(name: &'a str) -> Option<Self> {
        name.bytes()
            .enumerate()
            .rev()
            .filter(|&(_, c)| c == b'-')
            .find_map(|(pos, _)| {
                name.get(pos + 1..)
                    .and_then(|s| FullVersion::from_str(s).ok())
                    .and_then(|version| {
                        Some(Self {
                            name: name.get(..pos)?,
                            version,
                        })
                    })
            })
    }
}
