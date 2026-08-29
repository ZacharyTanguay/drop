use std::path::{Path, PathBuf};

use log::info;

use crate::error::ProcessError;

/// ZOUGCLOUD(ZC-003): Windows command lines are not POSIX shell words.
///
/// Upstream tokenises every launch command with `shell_words`, which applies
/// POSIX rules on all platforms. On Windows that is wrong twice over:
///
///  * backslash is an escape character in POSIX, so `C:\Users\Zack` collapses to
///    `C:UsersZack`;
///  * `shell_words::join` quotes with single quotes, which `cmd.exe` treats as
///    part of the filename rather than as quoting.
///
/// This module implements the quoting rules Windows users actually expect:
/// double quotes group, `""` is a literal quote inside a quoted run, and
/// backslash is an ordinary character.
///
/// Compiled under `test` on every platform so the rules stay covered by CI even
/// when it runs on Linux.
#[cfg(any(target_os = "windows", test))]
pub(crate) mod windows_words {
    /// Split a Windows command line into tokens.
    pub fn split(raw: &str) -> Vec<String> {
        let mut parts = Vec::new();
        let mut current = String::new();
        let mut in_quotes = false;
        // Tracked separately from `current.is_empty()` so that `""` yields an
        // empty argument rather than disappearing.
        let mut started = false;
        let mut chars = raw.chars().peekable();

        while let Some(c) = chars.next() {
            match c {
                '"' => {
                    started = true;
                    if in_quotes && chars.peek() == Some(&'"') {
                        chars.next();
                        current.push('"');
                    } else {
                        in_quotes = !in_quotes;
                    }
                }
                c if c.is_whitespace() && !in_quotes => {
                    if started {
                        parts.push(std::mem::take(&mut current));
                        started = false;
                    }
                }
                c => {
                    started = true;
                    current.push(c);
                }
            }
        }

        if started {
            parts.push(current);
        }

        parts
    }

    /// Quote a single token so `split` round-trips it.
    pub fn quote(part: &str) -> String {
        if !part.is_empty() && !part.contains([' ', '\t', '"']) {
            return part.to_owned();
        }

        let mut quoted = String::with_capacity(part.len() + 2);
        quoted.push('"');
        for c in part.chars() {
            if c == '"' {
                quoted.push('"');
            }
            quoted.push(c);
        }
        quoted.push('"');
        quoted
    }

    /// Join tokens back into a command line.
    pub fn join<I: IntoIterator<Item = String>>(parts: I) -> String {
        parts
            .into_iter()
            .map(|part| quote(&part))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug)]
pub struct ParsedCommand {
    pub env: Vec<String>,
    pub command: String,
    pub args: Vec<String>,
}

impl ParsedCommand {
    pub fn parse(raw: String) -> Result<Self, ProcessError> {
        let parts = Self::split_raw(&raw)?;
        let args =
            parts
                .iter()
                .position(|v| !v.contains("="))
                .ok_or(ProcessError::InvalidArguments(
                    "Cannot parse launch".to_owned(),
                ))?;
        let env = &parts[0..args];
        let command = parts[args].clone();
        let args = &parts[(args + 1)..];

        Ok(Self {
            args: args.to_vec(),
            command,
            env: env.to_vec(),
        })
    }

    // ZOUGCLOUD(ZC-003): platform-correct tokenising. Unix keeps the upstream
    // POSIX behaviour so Linux and macOS launch options are unaffected.
    #[cfg(target_os = "windows")]
    fn split_raw(raw: &str) -> Result<Vec<String>, ProcessError> {
        Ok(windows_words::split(raw))
    }

    #[cfg(not(target_os = "windows"))]
    fn split_raw(raw: &str) -> Result<Vec<String>, ProcessError> {
        shell_words::split(raw).map_err(|e| ProcessError::InvalidArguments(e.to_string()))
    }

    #[cfg(target_os = "windows")]
    fn join_parts(parts: Vec<String>) -> String {
        windows_words::join(parts)
    }

    #[cfg(not(target_os = "windows"))]
    fn join_parts(parts: Vec<String>) -> String {
        shell_words::join(parts)
    }

    pub fn make_absolute(&mut self, base: PathBuf) {
        self.command = base
            .join(self.command.clone())
            .to_string_lossy()
            .to_string();
    }

    /// ZOUGCLOUD(ZC-003): recover an unquoted executable whose name contains
    /// spaces.
    ///
    /// A Drop admin naturally types `Graveyard Keeper.exe`, because that is what
    /// the file is called. Any tokeniser splits that into two tokens, and no
    /// amount of quoting rules can tell "one file with a space" apart from
    /// "a command plus an argument" by syntax alone -- so we ask the filesystem.
    ///
    /// The command is only rewritten when the greedy candidate actually exists,
    /// which means `Game.exe --windowed` keeps its argument (`Game.exe` resolves
    /// first and we return immediately) and a PATH command such as `notepad`
    /// is left untouched for PATH resolution.
    ///
    /// Renaming the executable is not an option for the games this targets:
    /// Unity requires `Foo.exe` to sit next to `Foo_Data/`.
    pub fn coalesce_unquoted_command(&mut self, base: &Path) {
        if self.args.is_empty() || Self::resolves_to_file(base, &self.command) {
            return;
        }

        let mut candidate = self.command.clone();
        for index in 0..self.args.len() {
            candidate.push(' ');
            candidate.push_str(&self.args[index]);

            if Self::resolves_to_file(base, &candidate) {
                info!(
                    "coalesced unquoted command '{}' into '{}'",
                    self.command, candidate
                );
                self.command = candidate;
                self.args.drain(..=index);
                return;
            }
        }
    }

    fn resolves_to_file(base: &Path, command: &str) -> bool {
        let path = Path::new(command);
        if path.is_absolute() {
            path.is_file()
        } else {
            base.join(path).is_file()
        }
    }

    pub fn make_command_absolute_if_local(&mut self, base: &Path) {
        let candidate = base.join(&self.command);
        if candidate.is_file() {
            info!(
                "resolved local command '{}' to absolute path '{}'",
                self.command,
                candidate.display()
            );
            self.command = candidate.to_string_lossy().to_string();
        } else {
            info!(
                "command '{}' is not a local file in '{}', leaving as-is for PATH resolution",
                self.command,
                base.display()
            );
        }
    }

    pub fn ensure_executable(&self) -> Result<(), ProcessError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            if let Ok(metadata) = std::fs::metadata(&self.command) {
                let is_executable =
                    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0;
                if !is_executable {
                    return Err(ProcessError::NotExecutable(self.command.clone()));
                }
            }
        }

        Ok(())
    }

    pub fn reconstruct(self) -> String {
        let mut v = vec![];
        v.extend(self.env);
        v.extend_one(self.command);
        v.extend(self.args);
        Self::join_parts(v)
    }
}

pub struct LaunchParameters(pub ParsedCommand, pub PathBuf);

// ZOUGCLOUD(ZC-003): regression cover for the Windows launch-command rules.
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    mod tokenising {
        use super::super::windows_words;

        #[test]
        fn plain_command_and_argument() {
            assert_eq!(
                windows_words::split("Game.exe --windowed"),
                vec!["Game.exe", "--windowed"]
            );
        }

        #[test]
        fn quoted_command_with_spaces_stays_one_token() {
            assert_eq!(
                windows_words::split("\"Game With Spaces.exe\" --windowed"),
                vec!["Game With Spaces.exe", "--windowed"]
            );
        }

        #[test]
        fn backslashes_are_literal() {
            // The POSIX tokeniser upstream uses would turn this into C:UsersZack.
            assert_eq!(
                windows_words::split(r"C:\Users\Zack\Game.exe"),
                vec![r"C:\Users\Zack\Game.exe"]
            );
        }

        #[test]
        fn quoted_absolute_path_with_spaces() {
            assert_eq!(
                windows_words::split("\"C:\\Program Files\\My Game\\Game.exe\" -f"),
                vec!["C:\\Program Files\\My Game\\Game.exe", "-f"]
            );
        }

        #[test]
        fn doubled_quote_is_a_literal_quote() {
            assert_eq!(windows_words::split("\"a\"\"b\""), vec!["a\"b"]);
        }

        #[test]
        fn empty_quotes_produce_an_empty_argument() {
            assert_eq!(windows_words::split("Game.exe \"\""), vec!["Game.exe", ""]);
        }

        #[test]
        fn runs_of_whitespace_collapse() {
            assert_eq!(
                windows_words::split("  Game.exe   --a\t--b  "),
                vec!["Game.exe", "--a", "--b"]
            );
        }

        #[test]
        fn quote_only_when_needed() {
            assert_eq!(windows_words::quote("Game.exe"), "Game.exe");
            assert_eq!(windows_words::quote(r"C:\a\b.exe"), r"C:\a\b.exe");
            assert_eq!(
                windows_words::quote("Game With Spaces.exe"),
                "\"Game With Spaces.exe\""
            );
            assert_eq!(windows_words::quote(""), "\"\"");
        }

        #[test]
        fn join_then_split_round_trips() {
            for original in [
                vec!["Game.exe".to_owned()],
                vec!["Graveyard Keeper.exe".to_owned()],
                vec![r"C:\Program Files\My Game\Game.exe".to_owned(), "-f".to_owned()],
                vec!["cmd".to_owned(), "/C".to_owned(), r"C:\a b\run.bat".to_owned()],
                vec!["say \"hi\"".to_owned()],
            ] {
                let joined = windows_words::join(original.clone());
                assert_eq!(
                    windows_words::split(&joined),
                    original,
                    "round trip failed for {joined:?}"
                );
            }
        }

        #[test]
        fn single_quotes_are_not_quoting_on_windows() {
            // shell_words would strip these; cmd.exe would not.
            assert_eq!(
                windows_words::split("'Graveyard Keeper.exe'"),
                vec!["'Graveyard", "Keeper.exe'"]
            );
        }
    }

    fn parsed(command: &str, args: &[&str]) -> ParsedCommand {
        ParsedCommand {
            env: vec![],
            command: command.to_owned(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
        }
    }

    #[test]
    fn coalesce_recovers_unquoted_executable_with_spaces() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("Graveyard Keeper.exe"), b"x").expect("write");

        let mut command = parsed("Graveyard", &["Keeper.exe"]);
        command.coalesce_unquoted_command(dir.path());

        assert_eq!(command.command, "Graveyard Keeper.exe");
        assert!(command.args.is_empty());
    }

    #[test]
    fn coalesce_keeps_trailing_arguments() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("Graveyard Keeper.exe"), b"x").expect("write");

        let mut command = parsed("Graveyard", &["Keeper.exe", "--windowed"]);
        command.coalesce_unquoted_command(dir.path());

        assert_eq!(command.command, "Graveyard Keeper.exe");
        assert_eq!(command.args, vec!["--windowed"]);
    }

    #[test]
    fn coalesce_leaves_a_resolvable_command_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("Game.exe"), b"x").expect("write");

        let mut command = parsed("Game.exe", &["--windowed"]);
        command.coalesce_unquoted_command(dir.path());

        assert_eq!(command.command, "Game.exe");
        assert_eq!(command.args, vec!["--windowed"]);
    }

    #[test]
    fn coalesce_leaves_path_commands_alone() {
        let dir = tempfile::tempdir().expect("tempdir");

        let mut command = parsed("notepad", &["readme.txt"]);
        command.coalesce_unquoted_command(dir.path());

        assert_eq!(command.command, "notepad");
        assert_eq!(command.args, vec!["readme.txt"]);
    }

    #[test]
    fn coalesce_handles_an_absolute_command() {
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("Graveyard Keeper.exe");
        fs::write(&exe, b"x").expect("write");

        let first = dir.path().join("Graveyard").to_string_lossy().to_string();
        let mut command = parsed(&first, &["Keeper.exe"]);
        command.coalesce_unquoted_command(Path::new("/nonexistent-base"));

        assert_eq!(Path::new(&command.command), exe);
        assert!(command.args.is_empty());
    }
}
