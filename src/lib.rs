use core::{borrow::Borrow, cmp::Ordering, fmt};

/// The main part of a version number
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MainVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}
impl fmt::Display for MainVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}
impl fmt::Debug for MainVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <Self as fmt::Display>::fmt(self, f)
    }
}

/// The prerelease part of a version number. `T` should be an owned or borrowed string.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PreVersion<T> {
    pub stream: T,
    pub version: u16,
}
impl<T: Borrow<str>> PreVersion<T> {
    /// Borrows the stream name.
    pub fn borrow(&self) -> PreVersion<&str> {
        PreVersion {
            stream: self.stream.borrow(),
            version: self.version,
        }
    }
}
impl<T: ?Sized + ToOwned> PreVersion<&'_ T> {
    /// Converts the stream name to it's owned form.
    pub fn to_owned(&self) -> PreVersion<T::Owned> {
        PreVersion {
            stream: self.stream.to_owned(),
            version: self.version,
        }
    }
}

/// A version number with an optional pre-release part. `T` should be an owned or borrowed string.
#[derive(Clone, PartialEq, Eq)]
pub struct Version<T> {
    version: MainVersion,
    pre: Option<PreVersion<T>>,
}
impl<T: Borrow<str>> Version<T> {
    /// Borrows the pre-release stream name.
    pub fn borrow(&self) -> Version<&str> {
        Version {
            version: self.version,
            pre: self.pre.as_ref().map(|p| p.borrow()),
        }
    }
}
impl<T: ?Sized + ToOwned> Version<&'_ T> {
    /// Converts the pre-release stream name to it's owned form.
    pub fn to_owned(&self) -> Version<T::Owned> {
        Version {
            version: self.version,
            pre: self.pre.map(|p| p.to_owned()),
        }
    }
}
impl<'a> Version<&'a str> {
    /// Attempts to parse a version number from a string.
    pub fn parse(s: &'a str) -> Option<Self> {
        let mut iter = s.splitn(3, '.');
        let major = iter.next()?.parse().ok()?;
        let minor = iter.next()?.parse().ok()?;
        let s = iter.next()?;
        let (patch, pre) = match s.split_once('-') {
            Some((patch, pre)) => {
                let (stream, version) = pre.split_once('.')?;
                (
                    patch.parse().ok()?,
                    Some(PreVersion {
                        stream,
                        version: version.parse().ok()?,
                    }),
                )
            }
            None => (s.parse().ok()?, None),
        };
        Some(Self {
            version: MainVersion {
                major,
                minor,
                patch,
            },
            pre,
        })
    }
}
impl<T: fmt::Display> fmt::Display for Version<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.version.fmt(f)?;
        if let Some(pre) = &self.pre {
            write!(f, "-{}.{}", pre.stream, pre.version)?;
        }
        Ok(())
    }
}
impl<T: fmt::Display> fmt::Debug for Version<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <Self as fmt::Display>::fmt(self, f)
    }
}

/// Stores the latest stable version, as well as the latest prerelease version if it's newer than the latest stable version.
#[derive(Default)]
pub struct LatestVersions {
    stable: Option<MainVersion>,
    pre: Option<MainVersion>,
    pre_by_stream: Vec<PreVersion<String>>,
}
impl LatestVersions {
    /// Replaces the current version with the given version if it's newer.
    pub fn push(&mut self, arg: Version<&'_ str>) {
        if self.stable.map_or(true, |v| v > arg.version) {
            // current stable version is newer than the incoming version.
            return;
        }
        match arg.pre {
            Some(arg_pre) => {
                match self.pre.map_or(Ordering::Greater, |v| arg.version.cmp(&v)) {
                    // Incoming version is newer than the current prerelease version
                    Ordering::Greater => {
                        self.pre = Some(arg.version);
                        self.pre_by_stream.clear();
                        self.pre_by_stream.push(arg_pre.to_owned());
                    }
                    Ordering::Equal => {
                        // No way to tell which stream is newer; keep the newest version for each stream.
                        if let Some(pre) = self
                            .pre_by_stream
                            .iter_mut()
                            .find(|pre| arg_pre.stream == pre.stream)
                        {
                            pre.version = arg_pre.version.max(pre.version);
                        } else {
                            self.pre_by_stream.push(arg_pre.to_owned());
                        }
                    }
                    Ordering::Less => (),
                }
            }
            None => {
                self.stable = Some(arg.version);
                // Only keep pre-release versions if they're newer than the current stable version.
                if self.pre.map_or(false, |v| arg.version >= v) {
                    self.pre = None;
                    self.pre_by_stream.clear();
                }
            }
        }
    }

    /// Gets an iterator over all stable and pre-release versions.
    pub fn iter_ids<'a>(&'a self, name: &'a str) -> impl Iterator<Item = CrateId<'a>> {
        self.stable
            .into_iter()
            .map(move |version| CrateId {
                name,
                version: Version { version, pre: None },
            })
            .chain(self.pre.into_iter().flat_map(move |version| {
                self.pre_by_stream.iter().map(move |prerelease| CrateId {
                    name,
                    version: Version {
                        version,
                        pre: Some(prerelease.borrow()),
                    },
                })
            }))
    }
}

pub struct CrateId<'a> {
    pub name: &'a str,
    pub version: Version<&'a str>,
}
impl<'a> CrateId<'a> {
    pub fn parse(name: &'a str) -> Option<Self> {
        name.bytes()
            .enumerate()
            .rev()
            .filter(|&(_, c)| c == b'-')
            .find_map(|(pos, _)| {
                name.get(pos + 1..)
                    .and_then(Version::parse)
                    .and_then(|version| {
                        Some(Self {
                            name: name.get(..pos)?,
                            version,
                        })
                    })
            })
    }
}
impl fmt::Display for CrateId<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}-{}", self.name, self.version)
    }
}

/// Checks if string names an auto-published rustc crate. These no longer compile.
pub fn is_rustc_crate(name: &str) -> bool {
    name.starts_with("rustc-ap") | name.starts_with("fast-rustc-ap")
}
