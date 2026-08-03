use std::path::{Path, PathBuf};

use crate::error::Result;

#[cfg_attr(feature = "full-cli", derive(clap::Parser))]
#[cfg_attr(feature = "full-cli", command(version, about, long_about = None))]
#[derive(Debug, Clone)]
pub struct Args {
    /// Scanning root path
    #[cfg_attr(feature = "full-cli", arg(default_value = "."))]
    pub path: PathBuf,

    /// Maximum depth of tree to keep in memory.
    ///
    /// Subtrees below this depth are replaced with summary nodes.
    /// Does not affect scan depth.
    #[cfg_attr(feature = "full-cli", arg(short = 'd', long, default_value_t = 5))]
    pub max_depth: usize,

    /// Allow crossing filesystem boundarires.
    ///
    /// Use this on platforms without support for querying filesystem id.
    #[cfg_attr(feature = "full-cli", arg(long))]
    pub cross_fs: bool,

    /// Group files by type at the top-level, then split each region by directory.
    #[cfg_attr(feature = "full-cli", arg(short, long))]
    pub xray: bool,

    /// Don't *automatically* skip any files. Only overrides will be used.
    #[cfg_attr(feature = "full-cli", arg(short = 'A', long))]
    pub include_all: bool,

    /// Don't skip hidden files and folders
    #[cfg_attr(feature = "full-cli", arg(short = 'H', long))]
    pub include_hidden: bool,

    /// Don't skip .ignore'd files
    #[cfg_attr(feature = "full-cli", arg(short = 'I', long))]
    pub include_ignored: bool,

    /// Don't skip .gitignore'd files and folders
    #[cfg_attr(feature = "full-cli", arg(short = 'G', long))]
    pub include_gitignored: bool,

    /// Don't skip files and folders listed in .git/info/exclude
    #[cfg_attr(feature = "full-cli", arg(short = 'E', long))]
    pub include_gitexcluded: bool,

    /// Git-style override globs. '!' prefix negates glob
    pub overrides: Vec<String>,
}

impl Args {
    pub fn parse() -> Self {
        #[cfg(feature = "full-cli")]
        return <Self as clap::Parser>::parse();

        #[cfg(not(feature = "full-cli"))]
        match Self::parse_nano(std::env::args_os().skip(1)) {
            Ok(NanoAction::Run(args)) => args,
            Ok(NanoAction::Help(false)) => exit_with(0, SHORT_HELP.as_bytes()),
            Ok(NanoAction::Help(true)) => exit_with(0, LONG_HELP.as_bytes()),
            Ok(NanoAction::Version) => exit_with(
                0,
                concat!("leaves ", env!("CARGO_PKG_VERSION"), "\n").as_bytes(),
            ),
            Err(message) => {
                let message = format!(
                    "error: {message}\n\nUsage: leaves [OPTIONS] [PATH] [OVERRIDES]...\n\nFor more information, try '--help'.\n"
                );
                exit_with(2, message.as_bytes())
            }
        }
    }

    pub fn with_depth(&self, max_depth: usize) -> Self {
        Self {
            max_depth,
            ..self.clone()
        }
    }

    pub fn with_path(&self, path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            ..self.clone()
        }
    }

    #[cfg(not(feature = "full-cli"))]
    fn parse_nano(
        args: impl IntoIterator<Item = std::ffi::OsString>,
    ) -> std::result::Result<NanoAction, String> {
        let mut parsed = Self {
            path: PathBuf::from("."),
            max_depth: 5,
            cross_fs: false,
            xray: false,
            include_all: false,
            include_hidden: false,
            include_ignored: false,
            include_gitignored: false,
            include_gitexcluded: false,
            overrides: Vec::new(),
        };
        let mut args = args.into_iter();
        let mut positional_only = false;
        let mut path_set = false;

        while let Some(arg) = args.next() {
            let text = arg.to_str();
            if !positional_only {
                match text {
                    Some("--") => {
                        positional_only = true;
                        continue;
                    }
                    Some("-h") => return Ok(NanoAction::Help(false)),
                    Some("--help") => return Ok(NanoAction::Help(true)),
                    Some("-V" | "--version") => return Ok(NanoAction::Version),
                    Some("--cross-fs") => {
                        parsed.cross_fs = true;
                        continue;
                    }
                    Some("-x" | "--xray") => {
                        parsed.xray = true;
                        continue;
                    }
                    Some("-A" | "--include-all") => {
                        parsed.include_all = true;
                        continue;
                    }
                    Some("-H" | "--include-hidden") => {
                        parsed.include_hidden = true;
                        continue;
                    }
                    Some("-I" | "--include-ignored") => {
                        parsed.include_ignored = true;
                        continue;
                    }
                    Some("-G" | "--include-gitignored") => {
                        parsed.include_gitignored = true;
                        continue;
                    }
                    Some("-E" | "--include-gitexcluded") => {
                        parsed.include_gitexcluded = true;
                        continue;
                    }
                    Some(option @ ("-d" | "--max-depth")) => {
                        let value = args
                            .next()
                            .ok_or_else(|| format!("a value is required for '{option}'"))?;
                        parsed.max_depth = parse_depth(&value)?;
                        continue;
                    }
                    Some(value) if value.starts_with("--max-depth=") => {
                        parsed.max_depth =
                            parse_depth(std::ffi::OsStr::new(&value["--max-depth=".len()..]))?;
                        continue;
                    }
                    Some(value) if value.starts_with("--") => {
                        return Err(format!("unexpected argument '{value}'"));
                    }
                    Some(value) if value.starts_with('-') && value != "-" => {
                        if let Some(depth) = parse_short_options(value, &mut parsed)? {
                            parsed.max_depth = match depth {
                                Some(value) => parse_depth(std::ffi::OsStr::new(value))?,
                                None => {
                                    let value = args.next().ok_or_else(|| {
                                        "a value is required for '-d'".to_string()
                                    })?;
                                    parse_depth(&value)?
                                }
                            };
                        }
                        continue;
                    }
                    _ => {}
                }
            }

            if !path_set {
                parsed.path = PathBuf::from(arg);
                path_set = true;
            } else {
                parsed.overrides.push(
                    arg.into_string()
                        .map_err(|_| "override globs must be valid UTF-8".to_string())?,
                );
            }
        }

        Ok(NanoAction::Run(parsed))
    }
}

#[cfg(not(feature = "full-cli"))]
enum NanoAction {
    Run(Args),
    Help(bool),
    Version,
}

#[cfg(not(feature = "full-cli"))]
fn parse_depth(value: &std::ffi::OsStr) -> std::result::Result<usize, String> {
    value
        .to_str()
        .ok_or_else(|| "maximum depth must be valid UTF-8".to_string())?
        .parse()
        .map_err(|_| format!("invalid value {value:?} for '--max-depth'"))
}

#[cfg(not(feature = "full-cli"))]
fn parse_short_options<'a>(
    value: &'a str,
    parsed: &mut Args,
) -> std::result::Result<Option<Option<&'a str>>, String> {
    let mut chars = value[1..].char_indices().peekable();
    while let Some((offset, option)) = chars.next() {
        match option {
            'x' => parsed.xray = true,
            'A' => parsed.include_all = true,
            'H' => parsed.include_hidden = true,
            'I' => parsed.include_ignored = true,
            'G' => parsed.include_gitignored = true,
            'E' => parsed.include_gitexcluded = true,
            'd' => {
                let remainder = &value[offset + 2..];
                let remainder = remainder.strip_prefix('=').unwrap_or(remainder);
                return Ok(Some((!remainder.is_empty()).then_some(remainder)));
            }
            _ => return Err(format!("unexpected argument '-{option}'")),
        }
    }
    Ok(None)
}

#[cfg(not(feature = "full-cli"))]
fn exit_with(code: i32, message: &[u8]) -> ! {
    use std::io::Write as _;

    let result = if code == 0 {
        std::io::stdout().write_all(message)
    } else {
        std::io::stderr().write_all(message)
    };
    let _ = result;
    std::process::exit(code)
}

#[cfg(not(feature = "full-cli"))]
const SHORT_HELP: &str = "\
Usage: leaves [OPTIONS] [PATH] [OVERRIDES]...

Arguments:
  [PATH]          Scanning root path [default: .]
  [OVERRIDES]...  Git-style override globs. '!' prefix negates glob

Options:
  -d, --max-depth <MAX_DEPTH>  Maximum depth of tree to keep in memory [default: 5]
      --cross-fs               Allow crossing filesystem boundarires
  -x, --xray                   Group files by type at the top-level, then split each region by directory
  -A, --include-all            Don't *automatically* skip any files. Only overrides will be used
  -H, --include-hidden         Don't skip hidden files and folders
  -I, --include-ignored        Don't skip .ignore'd files
  -G, --include-gitignored     Don't skip .gitignore'd files and folders
  -E, --include-gitexcluded    Don't skip files and folders listed in .git/info/exclude
  -h, --help                   Print help (see more with '--help')
  -V, --version                Print version
";

#[cfg(not(feature = "full-cli"))]
const LONG_HELP: &str = "\
Usage: leaves [OPTIONS] [PATH] [OVERRIDES]...

Arguments:
  [PATH]
          Scanning root path

          [default: .]

  [OVERRIDES]...
          Git-style override globs. '!' prefix negates glob

Options:
  -d, --max-depth <MAX_DEPTH>
          Maximum depth of tree to keep in memory.

          Subtrees below this depth are replaced with summary nodes. Does not affect scan depth.

          [default: 5]

      --cross-fs
          Allow crossing filesystem boundarires.

          Use this on platforms without support for querying filesystem id.

  -x, --xray
          Group files by type at the top-level, then split each region by directory

  -A, --include-all
          Don't *automatically* skip any files. Only overrides will be used

  -H, --include-hidden
          Don't skip hidden files and folders

  -I, --include-ignored
          Don't skip .ignore'd files

  -G, --include-gitignored
          Don't skip .gitignore'd files and folders

  -E, --include-gitexcluded
          Don't skip files and folders listed in .git/info/exclude

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version
";

#[cfg(all(test, not(feature = "full-cli")))]
mod nano_parser_tests {
    use std::ffi::OsString;
    use std::path::Path;

    use super::{Args, NanoAction};

    fn parse(args: &[&str]) -> Args {
        let args = args.iter().map(OsString::from);
        match Args::parse_nano(args).unwrap() {
            NanoAction::Run(args) => args,
            _ => panic!("expected runnable arguments"),
        }
    }

    #[test]
    fn parses_flags_path_and_overrides() {
        let args = parse(&[
            "--max-depth=9",
            "-AHIGEx",
            "--cross-fs",
            "/tmp",
            "!target",
            "--",
            "-literal",
        ]);

        assert_eq!(args.path, Path::new("/tmp"));
        assert_eq!(args.max_depth, 9);
        assert!(args.cross_fs);
        assert!(args.xray);
        assert!(args.include_all);
        assert!(args.include_hidden);
        assert!(args.include_ignored);
        assert!(args.include_gitignored);
        assert!(args.include_gitexcluded);
        assert_eq!(args.overrides, ["!target", "-literal"]);
    }

    #[test]
    fn parses_attached_short_depth() {
        assert_eq!(parse(&["-d=7"]).max_depth, 7);
        assert_eq!(parse(&["-d8"]).max_depth, 8);
    }

    #[test]
    fn rejects_unknown_options() {
        let error = Args::parse_nano([OsString::from("--unknown")])
            .err()
            .unwrap();
        assert_eq!(error, "unexpected argument '--unknown'");
    }
}

pub fn init_logging() -> Result<()> {
    #[cfg(not(feature = "diagnostics"))]
    return Ok(());

    #[cfg(feature = "diagnostics")]
    {
        use tracing_subscriber::{EnvFilter, fmt::format::FmtSpan, prelude::*};

        let proj = env!("CARGO_CRATE_NAME").to_uppercase(); // need compile-time uppercase
        let Some(log_dir_env) = std::env::var_os(format!("{proj}_LOG_DIR")) else {
            return Ok(());
        };

        let log_dir = Path::new(&log_dir_env);
        std::fs::create_dir_all(log_dir)?;

        let log_path = log_dir.join("leaves.log");

        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?;

        let filter = EnvFilter::from_default_env();

        let file_subscriber = tracing_subscriber::fmt::layer()
            .with_span_events(FmtSpan::CLOSE)
            .with_file(true)
            .with_line_number(true)
            .with_writer(log_file)
            .with_target(false)
            .with_ansi(false)
            .with_filter(filter.clone());

        tracing_subscriber::registry()
            .with(file_subscriber)
            .with(filter)
            .init();

        Ok(())
    }
}
