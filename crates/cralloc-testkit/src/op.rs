//! Operations and scripts.
//!
//! An operation names its block by a [`BlockId`] given when the block is allocated, never by
//! position, so any subsequence of a script is a valid script: an operation on a block whose
//! allocation was removed is skipped. That is what lets [`crate::driver::minimize`] delete
//! operations freely.
//!
//! Scripts have a text form, one operation per `;`-separated item, which [`Script`] prints and
//! parses, so a failure report can be pasted into a test and replayed:
//!
//! | text | operation |
//! |---|---|
//! | `alloc ID SIZE ALIGN VARIANT`, `zalloc ...` | [`Op::Alloc`], zeroed for `zalloc` |
//! | `free ID VARIANT` | [`Op::Free`] |
//! | `resize ID SIZE ALIGN VARIANT`, `zresize ...` | [`Op::Resize`], zeroed for `zresize` |
//! | `touch ID` | [`Op::Touch`] |
//! | `send ID TO`, `recv FROM` | [`Op::Send`], [`Op::Recv`] |
//! | `migrate` | [`Op::Migrate`] |

use core::fmt;
use core::str::FromStr;

/// Names a block for the whole of a run. Workers draw from disjoint ranges.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(pub u64);

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One step of a stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Allocate a block and name it `id`.
    Alloc {
        /// Name of the new block.
        id: BlockId,
        /// Bytes requested.
        size: usize,
        /// Alignment requested.
        align: usize,
        /// Ask for zeroed memory.
        zeroed: bool,
        /// Chooses among equivalent entry points.
        variant: u32,
    },
    /// Check a block and free it.
    Free {
        /// The block.
        id: BlockId,
        /// Chooses among equivalent entry points.
        variant: u32,
    },
    /// Check a block and resize it.
    Resize {
        /// The block.
        id: BlockId,
        /// New size.
        size: usize,
        /// New alignment, where the interface lets a resize change it.
        align: usize,
        /// Ask for the new bytes to be zeroed, where the interface can.
        zeroed: bool,
        /// Chooses among equivalent entry points.
        variant: u32,
    },
    /// Check a block and write a fresh pattern into it, as a program uses its memory.
    Touch {
        /// The block.
        id: BlockId,
    },
    /// Check a block and hand it to another worker.
    Send {
        /// The block.
        id: BlockId,
        /// Receiving worker (interpreted by the hand-off pattern).
        to: u32,
    },
    /// Take blocks handed to this worker, checking each.
    Recv {
        /// Worker to steal from, for the work-stealing pattern.
        from: u32,
    },
    /// Continue the stream on a new thread and let this one exit.
    Migrate,
}

impl Op {
    /// The block the operation acts on, if any.
    pub fn block(&self) -> Option<BlockId> {
        match *self {
            Op::Alloc { id, .. }
            | Op::Free { id, .. }
            | Op::Resize { id, .. }
            | Op::Touch { id }
            | Op::Send { id, .. } => Some(id),
            Op::Recv { .. } | Op::Migrate => None,
        }
    }
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let z = |zeroed: bool| if zeroed { "z" } else { "" };
        match *self {
            Op::Alloc {
                id,
                size,
                align,
                zeroed,
                variant,
            } => write!(f, "{}alloc {id} {size} {align} {variant}", z(zeroed)),
            Op::Free { id, variant } => write!(f, "free {id} {variant}"),
            Op::Resize {
                id,
                size,
                align,
                zeroed,
                variant,
            } => write!(f, "{}resize {id} {size} {align} {variant}", z(zeroed)),
            Op::Touch { id } => write!(f, "touch {id}"),
            Op::Send { id, to } => write!(f, "send {id} {to}"),
            Op::Recv { from } => write!(f, "recv {from}"),
            Op::Migrate => write!(f, "migrate"),
        }
    }
}

/// Why a script did not parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

impl FromStr for Op {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Op, ParseError> {
        let mut words = s.split_whitespace();
        let keyword = words
            .next()
            .ok_or_else(|| ParseError("empty operation".into()))?;
        let fields: Vec<u64> = words
            .map(|w| {
                w.parse()
                    .map_err(|_| ParseError(format!("bad number {w:?} in {s:?}")))
            })
            .collect::<Result<_, _>>()?;
        let want = |n: usize| {
            if fields.len() == n {
                Ok(())
            } else {
                Err(ParseError(format!("{keyword} takes {n} numbers: {s:?}")))
            }
        };
        let size = |x: u64| usize::try_from(x).map_err(|_| ParseError(format!("{x} too large")));
        let small = |x: u64| u32::try_from(x).map_err(|_| ParseError(format!("{x} too large")));
        let (zeroed, base) = match keyword.strip_prefix('z') {
            Some(rest @ ("alloc" | "resize")) => (true, rest),
            _ => (false, keyword),
        };
        Ok(match base {
            "alloc" | "resize" => {
                want(4)?;
                let (id, size, align, variant) = (
                    BlockId(fields[0]),
                    size(fields[1])?,
                    size(fields[2])?,
                    small(fields[3])?,
                );
                if !align.is_power_of_two() {
                    return Err(ParseError(format!(
                        "alignment {align} is not a power of two"
                    )));
                }
                if base == "alloc" {
                    Op::Alloc {
                        id,
                        size,
                        align,
                        zeroed,
                        variant,
                    }
                } else {
                    Op::Resize {
                        id,
                        size,
                        align,
                        zeroed,
                        variant,
                    }
                }
            }
            "free" => {
                want(2)?;
                Op::Free {
                    id: BlockId(fields[0]),
                    variant: small(fields[1])?,
                }
            }
            "touch" => {
                want(1)?;
                Op::Touch {
                    id: BlockId(fields[0]),
                }
            }
            "send" => {
                want(2)?;
                Op::Send {
                    id: BlockId(fields[0]),
                    to: small(fields[1])?,
                }
            }
            "recv" => {
                want(1)?;
                Op::Recv {
                    from: small(fields[0])?,
                }
            }
            "migrate" => {
                want(0)?;
                Op::Migrate
            }
            _ => return Err(ParseError(format!("unknown operation {keyword:?}"))),
        })
    }
}

/// A sequence of operations that replays exactly.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Script(pub Vec<Op>);

impl fmt::Display for Script {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, op) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{op}")?;
        }
        Ok(())
    }
}

impl FromStr for Script {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Script, ParseError> {
        s.split([';', '\n'])
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map(Script)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_print_and_parse_back() {
        let ops = vec![
            Op::Alloc {
                id: BlockId(1),
                size: 24,
                align: 8,
                zeroed: false,
                variant: 7,
            },
            Op::Alloc {
                id: BlockId(2),
                size: 0,
                align: 4096,
                zeroed: true,
                variant: 0,
            },
            Op::Resize {
                id: BlockId(1),
                size: 100,
                align: 16,
                zeroed: true,
                variant: 3,
            },
            Op::Touch { id: BlockId(2) },
            Op::Send {
                id: BlockId(2),
                to: 3,
            },
            Op::Recv { from: 1 },
            Op::Migrate,
            Op::Free {
                id: BlockId(1),
                variant: u32::MAX,
            },
        ];
        let script = Script(ops);
        let text = script.to_string();
        assert_eq!(text.parse::<Script>(), Ok(script.clone()), "{text}");
        assert!(text.starts_with("alloc 1 24 8 7; zalloc 2 0 4096 0; zresize 1 100 16 3"));
        let multiline = text.replace("; ", "\n");
        assert_eq!(multiline.parse::<Script>(), Ok(script));
    }

    #[test]
    fn malformed_operations_are_rejected() {
        for bad in [
            "alloc 1 2 3",
            "alloc 1 24 3 0",
            "frob 1",
            "free x 0",
            "migrate 1",
        ] {
            assert!(bad.parse::<Op>().is_err(), "{bad}");
        }
        assert_eq!("".parse::<Script>(), Ok(Script::default()));
    }
}
