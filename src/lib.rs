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
    build: Option<T>,
}
impl<T: Borrow<str>> Version<T> {
    /// Borrows the pre-release stream name.
    pub fn borrow(&self) -> Version<&str> {
        Version {
            version: self.version,
            pre: self.pre.as_ref().map(|p| p.borrow()),
            build: self.build.as_ref().map(|b| b.borrow()),
        }
    }
}
impl<T: ?Sized + ToOwned> Version<&'_ T> {
    /// Converts the pre-release stream name to it's owned form.
    pub fn to_owned(&self) -> Version<T::Owned> {
        Version {
            version: self.version,
            pre: self.pre.map(|p| p.to_owned()),
            build: self.build.map(|b| b.to_owned()),
        }
    }
}
impl<'a> Version<&'a str> {
    /// Attempts to parse a version number from a string.
    pub fn parse(s: &'a str) -> Option<Self> {
        fn parse_with_build(s: &str) -> Option<(u16, Option<&str>)> {
            let (s, build) = if let Some((s, build)) = s.split_once('+') {
                (s, Some(build))
            } else {
                (s, None)
            };
            s.parse().ok().map(|v| (v, build))
        }

        let mut iter = s.splitn(3, '.');
        let major = iter.next()?.parse().ok()?;
        let minor = iter.next()?.parse().ok()?;
        let s = iter.next()?;
        match s.split_once('-') {
            Some((patch, pre)) => {
                let (stream, version) = pre.split_once('.')?;
                let (version, build) = parse_with_build(version)?;
                Some(Self {
                    version: MainVersion {
                        major,
                        minor,
                        patch: patch.parse().ok()?,
                    },
                    pre: Some(PreVersion { stream, version }),
                    build,
                })
            }
            None => {
                let (patch, build) = parse_with_build(s)?;
                Some(Self {
                    version: MainVersion {
                        major,
                        minor,
                        patch,
                    },
                    pre: None,
                    build,
                })
            }
        }
    }
}
impl<T: fmt::Display> fmt::Display for Version<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.version.fmt(f)?;
        if let Some(pre) = &self.pre {
            write!(f, "-{}.{}", pre.stream, pre.version)?;
        }
        if let Some(build) = &self.build {
            write!(f, "+{}", build)?;
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
    stable: Option<(MainVersion, Option<String>)>,
    pre: Option<MainVersion>,
    pre_by_stream: Vec<(PreVersion<String>, Option<String>)>,
}
impl LatestVersions {
    /// Replaces the current version with the given version if it's newer.
    pub fn push(&mut self, arg: Version<&'_ str>) {
        if self
            .stable
            .as_ref()
            .map_or(false, |&(v, _)| v >= arg.version)
        {
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
                        self.pre_by_stream
                            .push((arg_pre.to_owned(), arg.build.map(|x| x.to_owned())));
                    }
                    Ordering::Equal => {
                        // No way to tell which stream is newer; keep the newest version for each stream.
                        if let Some((pre, build)) = self
                            .pre_by_stream
                            .iter_mut()
                            .find(|(pre, _)| arg_pre.stream == pre.stream)
                        {
                            if arg_pre.version > pre.version {
                                pre.version = arg_pre.version;
                                *build = arg.build.map(|x| x.to_owned());
                            }
                        } else {
                            self.pre_by_stream
                                .push((arg_pre.to_owned(), arg.build.map(|x| x.to_owned())));
                        }
                    }
                    Ordering::Less => (),
                }
            }
            None => {
                self.stable = Some((arg.version, arg.build.map(|x| x.to_owned())));
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
            .iter()
            .map(move |&(version, ref build)| CrateId {
                name,
                version: Version {
                    version,
                    pre: None,
                    build: build.as_deref(),
                },
            })
            .chain(self.pre.into_iter().flat_map(move |version| {
                self.pre_by_stream
                    .iter()
                    .map(move |(prerelease, build)| CrateId {
                        name,
                        version: Version {
                            version,
                            pre: Some(prerelease.borrow()),
                            build: build.as_deref(),
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

#[cfg(test)]
mod test {
    use super::{LatestVersions, MainVersion, PreVersion, Version};

    macro_rules! version {
        (@opt) => {
            None
        };
        (@opt $stream:ident:$version:literal) => {
            Some(PreVersion {
                stream: stringify!($stream),
                version: $version,
            })
        };
        (@opt $build:literal) => {
            Some($build)
        };

        ($major:literal:$minor:literal:$patch:literal $(- $stream:ident:$version:literal)? $(+ $build:literal)?) => {
            Version {
                version: MainVersion {
                    major: $major,
                    minor: $minor,
                    patch: $patch,
                },
                pre: version!(@opt $($stream:$version)?),
                build: version!(@opt $($build)?),
            }
        };
    }

    #[test]
    fn parse_version() {
        assert_eq!(Version::parse("0.0.0").unwrap(), version!(0:0:0));
        assert_eq!(Version::parse("1.9.0").unwrap(), version!(1:9:0));
        assert_eq!(
            Version::parse("1.0.0-beta.1").unwrap(),
            version!(1:0:0-beta:1)
        );
        assert_eq!(
            Version::parse("9.9.52-alphastar.999").unwrap(),
            version!(9:9:52-alphastar:999)
        );
        assert_eq!(
            Version::parse("1.0.0+someotherstuff.2020.5.2").unwrap(),
            version!(1:0:0+"someotherstuff.2020.5.2")
        );
        assert_eq!(
            Version::parse("0.1.0-beta.5+build.2020.5.2").unwrap(),
            version!(0:1:0-beta:5+"build.2020.5.2")
        );
    }

    #[test]
    fn latest_versions() {
        let mut versions = LatestVersions::default();
        versions.push(version!(0:0:1));

        assert_eq!(
            versions.iter_ids("").map(|x| x.version).collect::<Vec<_>>(),
            [version!(0:0:1)].as_slice()
        );

        versions.push(version!(0:0:2));
        assert_eq!(
            versions.iter_ids("").map(|x| x.version).collect::<Vec<_>>(),
            [version!(0:0:2)].as_slice()
        );

        versions.push(version!(0:0:1));
        assert_eq!(
            versions.iter_ids("").map(|x| x.version).collect::<Vec<_>>(),
            [version!(0:0:2)].as_slice()
        );

        versions.push(version!(0:0:2));
        assert_eq!(
            versions.iter_ids("").map(|x| x.version).collect::<Vec<_>>(),
            [version!(0:0:2)].as_slice()
        );

        versions.push(version!(0:0:1+"build.1.2"));
        assert_eq!(
            versions.iter_ids("").map(|x| x.version).collect::<Vec<_>>(),
            [version!(0:0:2)].as_slice()
        );

        versions.push(version!(0:1:0+"build.1.2"));
        assert_eq!(
            versions.iter_ids("").map(|x| x.version).collect::<Vec<_>>(),
            [version!(0:1:0+"build.1.2")].as_slice()
        );

        versions.push(version!(0:0:999));
        assert_eq!(
            versions.iter_ids("").map(|x| x.version).collect::<Vec<_>>(),
            [version!(0:1:0+"build.1.2")].as_slice()
        );

        versions.push(version!(1:0:0));
        versions.push(version!(0:9:0-beta:1));
        versions.push(version!(0:9:0-beta:1+"build.1"));
        versions.push(version!(1:0:0-rc:1));
        assert_eq!(
            versions.iter_ids("").map(|x| x.version).collect::<Vec<_>>(),
            [version!(1:0:0)].as_slice()
        );

        versions.push(version!(1:1:0-rc:1));
        assert_eq!(
            versions.iter_ids("").map(|x| x.version).collect::<Vec<_>>(),
            [version!(1:0:0), version!(1:1:0-rc:1)].as_slice()
        );

        versions.push(version!(1:1:0-rc:2));
        assert_eq!(
            versions.iter_ids("").map(|x| x.version).collect::<Vec<_>>(),
            [version!(1:0:0), version!(1:1:0-rc:2)].as_slice()
        );

        versions.push(version!(1:1:0-rc:1));
        assert_eq!(
            versions.iter_ids("").map(|x| x.version).collect::<Vec<_>>(),
            [version!(1:0:0), version!(1:1:0-rc:2)].as_slice()
        );

        versions.push(version!(1:1:0-beta:1));
        assert_eq!(
            versions.iter_ids("").map(|x| x.version).collect::<Vec<_>>(),
            [
                version!(1:0:0),
                version!(1:1:0-rc:2),
                version!(1:1:0-beta:1),
            ]
            .as_slice()
        );

        versions.push(version!(1:1:0-beta:2));
        versions.push(version!(1:1:0-rc:3+"build.1"));
        assert_eq!(
            versions.iter_ids("").map(|x| x.version).collect::<Vec<_>>(),
            [
                version!(1:0:0),
                version!(1:1:0-rc:3+"build.1"),
                version!(1:1:0-beta:2),
            ]
            .as_slice()
        );

        versions.push(version!(1:1:0-rc:4+"build.9"));
        assert_eq!(
            versions.iter_ids("").map(|x| x.version).collect::<Vec<_>>(),
            [
                version!(1:0:0),
                version!(1:1:0-rc:4+"build.9"),
                version!(1:1:0-beta:2),
            ]
            .as_slice()
        );

        versions.push(version!(1:1:0));
        assert_eq!(
            versions.iter_ids("").map(|x| x.version).collect::<Vec<_>>(),
            [version!(1:1:0)].as_slice()
        );

        versions.push(version!(1:2:0-beta:1));
        versions.push(version!(1:2:0-rc:1));
        assert_eq!(
            versions.iter_ids("").map(|x| x.version).collect::<Vec<_>>(),
            [
                version!(1:1:0),
                version!(1:2:0-beta:1),
                version!(1:2:0-rc:1),
            ]
            .as_slice()
        );

        versions.push(version!(1:3:0-rc:1));
        assert_eq!(
            versions.iter_ids("").map(|x| x.version).collect::<Vec<_>>(),
            [version!(1:1:0), version!(1:3:0-rc:1)].as_slice()
        );

        versions.push(version!(1:2:0));
        assert_eq!(
            versions.iter_ids("").map(|x| x.version).collect::<Vec<_>>(),
            [version!(1:2:0), version!(1:3:0-rc:1)].as_slice()
        );

        versions.push(version!(0:9:0));
        assert_eq!(
            versions.iter_ids("").map(|x| x.version).collect::<Vec<_>>(),
            [version!(1:2:0), version!(1:3:0-rc:1)].as_slice()
        );
    }
}
