//! What a vitrum process's exit code means.
//!
//! One table, shared by both binaries. Not part of the wire protocol, and it
//! lives here for the same reason the wire contract does: `vitrum` and
//! `vitrum-server` are installed as a pair, scripts branch on both, and this
//! crate is the one thing they already agree on. Two tables would be two
//! answers to "what does 3 mean", and the second one would be discovered by an
//! operator whose installer took the wrong branch.
//!
//! The numbers are the product's contract with a shell, so they are chosen for
//! what a caller can DO about them rather than for where in the code the
//! failure happened:
//!
//! - [`Exit::Usage`] means the command line was wrong. Retrying it changes
//!   nothing; the caller has to be edited.
//! - [`Exit::Unavailable`] means the command was right and this machine is not
//!   in a state where it can work. Retrying later, after the missing thing is
//!   installed or the port is freed, can succeed.
//! - [`Exit::Offline`] means a remote endpoint could not be reached. Retrying
//!   later, unchanged, is exactly the right response.
//! - [`Exit::Corrupt`] means bytes did not match the digest published for them.
//!   Retrying is right too, but a second failure is a supply-chain problem and
//!   not a flaky link, which is why it is not folded into [`Exit::Offline`].
//! - [`Exit::Failed`] is everything else that was understood and did not
//!   complete.
//!
//! Every `--help` in the product renders its own subset of this table through
//! [`status_lines`], so the documentation cannot drift from the enum.

/// The exit code a vitrum process may return.
///
/// `#[repr(i32)]` with explicit discriminants: the numbers ARE the contract,
/// so they are written where a reader looking for them will find them, and
/// they never shift when a variant is added in the middle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(i32)]
pub enum Exit {
    /// It worked. Also "nothing needed doing", which is not a failure.
    Ok = 0,
    /// Understood, and it did not complete.
    Failed = 1,
    /// The command line was wrong. Nothing was attempted.
    Usage = 2,
    /// This machine cannot do it right now: something is not installed, a port
    /// is taken, a directory is not writable, or no build exists for it.
    Unavailable = 3,
    /// A remote endpoint could not be reached.
    Offline = 4,
    /// Bytes did not match the digest published for them.
    Corrupt = 5,
}

impl Exit {
    /// Every code, in numeric order.
    ///
    /// Exhaustive by construction: [`Exit::code`] matches on `self`, so a new
    /// variant that is not added here fails `every_code_is_in_the_table`.
    pub const ALL: &'static [Exit] = &[
        Exit::Ok,
        Exit::Failed,
        Exit::Usage,
        Exit::Unavailable,
        Exit::Offline,
        Exit::Corrupt,
    ];

    /// The number the process exits with.
    pub const fn code(self) -> i32 {
        self as i32
    }

    /// The one-line meaning, as `--help` prints it.
    ///
    /// Phrased for somebody reading a script's `if` rather than for somebody
    /// reading this file, so it says what the caller may do about the code and
    /// not which function produced it.
    pub const fn meaning(self) -> &'static str {
        match self {
            Exit::Ok => "it worked, or nothing needed doing",
            Exit::Failed => "understood, and it did not complete",
            Exit::Usage => "the command line was wrong; nothing was attempted",
            Exit::Unavailable => "this machine cannot do it yet; fix that and retry",
            Exit::Offline => "a remote endpoint could not be reached; retry later",
            Exit::Corrupt => "bytes did not match the digest published for them",
        }
    }

    /// The code, if `code` is one this product returns.
    pub fn from_code(code: i32) -> Option<Exit> {
        Exit::ALL.iter().copied().find(|e| e.code() == code)
    }
}

/// Width the code column is padded to in help text.
///
/// Wide enough that the meanings line up under the option descriptions every
/// `--help` in the product already prints in the same two-column shape.
const CODE_COLUMN: usize = 21;

/// The `exit status:` block a `--help` prints, for the codes it can return.
///
/// A subset, never the whole table, because a command that cannot reach the
/// network must not tell an operator to expect 4 from it. The caller passes
/// exactly what it returns and
/// `crates/vitrum-proto/src/exit.rs::help_documents_every_code_it_returns`
/// checks that against the source of each command.
///
/// Duplicates collapse and the order is numeric regardless of the order given,
/// so a caller listing its codes in the order its own code produces them still
/// prints a table in the shape a reader expects.
pub fn status_lines(codes: &[Exit]) -> String {
    let mut wanted: Vec<Exit> = Exit::ALL
        .iter()
        .copied()
        .filter(|e| codes.contains(e))
        .collect();
    wanted.dedup();
    let mut out = String::new();
    for code in wanted {
        let number = code.code().to_string();
        out.push_str("  ");
        out.push_str(&number);
        for _ in number.len()..CODE_COLUMN {
            out.push(' ');
        }
        out.push_str(code.meaning());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The numbers are the contract with every script that calls this product,
    /// so they are pinned here rather than left to declaration order.
    ///
    /// A variant inserted in the middle without a discriminant would silently
    /// renumber everything after it, and an installer's `if [ $? -eq 3 ]`
    /// would start taking the wrong branch on the next release.
    #[test]
    fn the_numbers_are_pinned() {
        assert_eq!(Exit::Ok.code(), 0);
        assert_eq!(Exit::Failed.code(), 1);
        assert_eq!(Exit::Usage.code(), 2);
        assert_eq!(Exit::Unavailable.code(), 3);
        assert_eq!(Exit::Offline.code(), 4);
        assert_eq!(Exit::Corrupt.code(), 5);
    }

    /// Every variant appears in [`Exit::ALL`], and no number is used twice.
    ///
    /// Derived from the source rather than from a hand-written list: the count
    /// is read out of this file's own enum, so adding a seventh variant and
    /// forgetting `ALL` turns this red instead of quietly shrinking the table
    /// that `--help` and `from_code` are both built on.
    #[test]
    fn every_code_is_in_the_table() {
        let src = include_str!("exit.rs");
        let body = src
            .split("pub enum Exit {")
            .nth(1)
            .and_then(|rest| rest.split("\n}").next())
            .expect("the enum is declared in this file");
        let declared = body
            .lines()
            .filter_map(|l| l.trim().strip_suffix(','))
            .filter(|l| l.contains(" = "))
            .count();
        assert_eq!(
            declared,
            Exit::ALL.len(),
            "a variant was added to Exit without being added to Exit::ALL"
        );

        let mut codes: Vec<i32> = Exit::ALL.iter().map(|e| e.code()).collect();
        let before = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(before, codes.len(), "two variants share one exit code");
        assert_eq!(
            codes,
            (0..before as i32).collect::<Vec<_>>(),
            "the codes must be contiguous from zero, so a table has no holes"
        );
    }

    /// A code read back off a shell is the variant that produced it.
    #[test]
    fn a_number_round_trips_to_its_meaning() {
        for code in Exit::ALL {
            assert_eq!(Exit::from_code(code.code()), Some(*code));
        }
        assert_eq!(Exit::from_code(6), None);
        assert_eq!(Exit::from_code(-1), None);
    }

    /// The rendered block names each code once, in numeric order, with a
    /// meaning beside it, whatever order the caller listed them in.
    #[test]
    fn the_help_block_is_ordered_and_deduplicated() {
        let text = status_lines(&[Exit::Offline, Exit::Ok, Exit::Usage, Exit::Ok]);
        let numbers: Vec<&str> = text
            .lines()
            .map(|l| l.trim_start().split_whitespace().next().unwrap_or(""))
            .collect();
        assert_eq!(numbers, ["0", "2", "4"]);
        assert!(text.contains(Exit::Offline.meaning()), "{text}");
        assert!(!text.contains('{'), "an unrendered placeholder: {text}");
    }

    /// Zero is never a failure and a failure is never zero.
    ///
    /// Trivial to state and the thing that was actually broken: `vitrum
    /// --bogus` printed its usage to stdout and exited 0, so a script that
    /// mistyped a flag saw a successful launch.
    #[test]
    fn only_ok_is_zero() {
        for code in Exit::ALL {
            assert_eq!(
                code.code() == 0,
                *code == Exit::Ok,
                "{code:?} disagrees with success"
            );
        }
    }
}
