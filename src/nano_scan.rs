use std::borrow::Cow;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use glob_set::{Glob, GlobMap, GlobMapBuilder};

use super::ScanState;
use crate::cli::Args;
use crate::colors::ColorScheme;
use crate::core::Entry;
use crate::error::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Decision {
    Include,
    Exclude,
}

#[derive(Clone, Debug)]
struct Rule {
    #[cfg(test)]
    base: PathBuf,
    pattern: String,
    decision: Decision,
    dir_only: bool,
    basename_only: bool,
}

impl Rule {
    #[cfg(test)]
    fn matches(&self, path: &Path, is_dir: bool) -> bool {
        if self.dir_only && !is_dir {
            return false;
        }

        let Ok(relative) = path.strip_prefix(&self.base) else {
            return false;
        };
        if relative.as_os_str().is_empty() {
            return false;
        }

        if self.basename_only {
            return path
                .file_name()
                .and_then(|name| Glob::new(&self.pattern).ok().map(|glob| (name, glob)))
                .is_some_and(|(name, glob)| {
                    glob.compile_matcher().is_match(name.to_string_lossy())
                });
        }

        Glob::new(&self.pattern).is_ok_and(|glob| {
            glob.compile_matcher()
                .is_match(slash_path(relative).as_ref())
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct RuleMatch {
    decision: Decision,
    priority: usize,
}

#[derive(Debug)]
struct CompiledRules {
    basenames: GlobMap<RuleMatch>,
    paths: GlobMap<RuleMatch>,
}

impl CompiledRules {
    fn new(rules: &[Rule], include_dir_only: bool) -> std::result::Result<Self, glob_set::Error> {
        let mut basenames = GlobMapBuilder::new();
        let mut paths = GlobMapBuilder::new();
        let mut seen_patterns = Vec::new();

        // GlobMap returns the lowest-index match, so reverse insertion preserves
        // gitignore's "last matching rule wins" precedence.
        for (priority, rule) in rules.iter().enumerate().rev() {
            if rule.dir_only && !include_dir_only {
                continue;
            }
            // glob-set's literal fast path stores one entry per exact pattern.
            // Keep only the newest duplicate so that fast path cannot reverse
            // gitignore precedence.
            if seen_patterns.contains(&rule.pattern.as_str()) {
                continue;
            }
            seen_patterns.push(rule.pattern.as_str());
            let Ok(glob) = Glob::new(&rule.pattern) else {
                continue;
            };
            let matched = RuleMatch {
                decision: rule.decision,
                priority,
            };
            if rule.basename_only {
                basenames.insert(glob, matched);
            } else {
                paths.insert(glob, matched);
            }
        }

        Ok(Self {
            basenames: basenames.build()?,
            paths: paths.build()?,
        })
    }

    fn matched(&self, basename: &str, relative: &str) -> Option<Decision> {
        match (self.basenames.get(basename), self.paths.get(relative)) {
            (Some(left), Some(right)) => Some(
                if left.priority > right.priority {
                    left
                } else {
                    right
                }
                .decision,
            ),
            (Some(matched), None) | (None, Some(matched)) => Some(matched.decision),
            (None, None) => None,
        }
    }
}

#[derive(Debug)]
struct RuleScope {
    base: PathBuf,
    directories: CompiledRules,
    files: CompiledRules,
}

impl RuleScope {
    fn new(base: &Path, rules: &[Rule]) -> std::result::Result<Self, glob_set::Error> {
        Ok(Self {
            base: base.to_path_buf(),
            directories: CompiledRules::new(rules, true)?,
            files: CompiledRules::new(rules, false)?,
        })
    }

    fn matched(&self, path: &Path, is_dir: bool) -> Option<Decision> {
        let relative = path.strip_prefix(&self.base).ok()?;
        if relative.as_os_str().is_empty() {
            return None;
        }
        let basename = path.file_name()?.to_string_lossy();
        let relative = slash_path(relative);
        if is_dir {
            self.directories.matched(&basename, &relative)
        } else {
            self.files.matched(&basename, &relative)
        }
    }
}

#[derive(Clone, Default)]
struct RuleSet {
    scopes: Vec<Arc<RuleScope>>,
}

impl RuleSet {
    fn matched(&self, path: &Path, is_dir: bool) -> Option<Decision> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.matched(path, is_dir))
    }

    fn add_rules(
        &mut self,
        base: &Path,
        rules: &[Rule],
    ) -> std::result::Result<(), glob_set::Error> {
        if !rules.is_empty() {
            self.scopes.push(Arc::new(RuleScope::new(base, rules)?));
        }
        Ok(())
    }

    fn add_file(&mut self, path: &Path, base: &Path) {
        let Ok(bytes) = fs::read(path) else {
            return;
        };
        let text = String::from_utf8_lossy(&bytes);
        let rules = text
            .lines()
            .filter_map(|line| parse_rule(line, base, false))
            .collect::<Vec<_>>();
        let _ = self.add_rules(base, &rules);
    }
}

#[derive(Clone, Default)]
struct Rules {
    ignore: RuleSet,
    gitignore: RuleSet,
    git_exclude: RuleSet,
    git_global: RuleSet,
}

impl Rules {
    fn matched(&self, path: &Path, is_dir: bool, in_git: bool) -> Option<Decision> {
        self.ignore
            .matched(path, is_dir)
            .or_else(|| {
                in_git
                    .then(|| self.gitignore.matched(path, is_dir))
                    .flatten()
            })
            .or_else(|| {
                in_git
                    .then(|| self.git_exclude.matched(path, is_dir))
                    .flatten()
            })
            .or_else(|| {
                in_git
                    .then(|| self.git_global.matched(path, is_dir))
                    .flatten()
            })
    }
}

#[derive(Clone)]
struct Overrides {
    rules: RuleSet,
    has_includes: bool,
}

impl Overrides {
    fn new(args: &Args) -> Result<Self> {
        let mut rules = RuleSet::default();
        let mut has_includes = false;
        let mut parsed = Vec::new();
        for pattern in &args.overrides {
            validate_override(pattern)?;
            if let Some(rule) = parse_rule(pattern, &args.path, true) {
                has_includes |= rule.decision == Decision::Include;
                parsed.push(rule);
            }
        }
        rules.add_rules(&args.path, &parsed)?;
        Ok(Self {
            rules,
            has_includes,
        })
    }

    fn matched(&self, path: &Path, is_dir: bool) -> Option<Decision> {
        self.rules.matched(path, is_dir)
    }
}

#[derive(Clone)]
struct Repository {
    root: PathBuf,
    git_dir: PathBuf,
}

struct WalkTask {
    path: PathBuf,
    file_type: fs::FileType,
}

struct DirectoryTask {
    walker: Walker,
    dir: PathBuf,
}

enum QueueMessage {
    Directory(DirectoryTask),
    Stop,
}

#[derive(Clone)]
struct Walker {
    colors: ColorScheme,
    args: Args,
    state: Arc<Mutex<ScanState>>,
    tx: mpsc::Sender<Entry>,
    overrides: Overrides,
    rules: Rules,
    repository: Option<Repository>,
    root_device: Option<u64>,
}

pub(super) fn spawn_walker(
    colors: &ColorScheme,
    args: &Args,
    state: Arc<Mutex<ScanState>>,
    root: impl AsRef<Path>,
) -> Result<mpsc::Receiver<Entry>> {
    let root = root.as_ref().to_path_buf();
    let (tx, rx) = mpsc::channel();
    let mut walker = Walker {
        colors: colors.clone(),
        args: args.clone(),
        state,
        tx,
        overrides: Overrides::new(args)?,
        rules: Rules::default(),
        repository: find_repository(&root),
        root_device: device_id(&root),
    };
    walker.load_initial_rules(&root);

    std::thread::spawn(move || {
        if root.is_dir() {
            let _ = walker.walk_root_parallel(&root);
        } else {
            let _ = walker.walk_root_file(&root);
        }
    });
    Ok(rx)
}

impl Walker {
    fn walk_root_parallel(&mut self, dir: &Path) -> bool {
        let worker_count = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1);
        let (queue_tx, queue_rx) = crossbeam_channel::unbounded();
        let pending = AtomicUsize::new(1);
        let cancelled = AtomicBool::new(false);
        queue_tx
            .send(QueueMessage::Directory(DirectoryTask {
                walker: self.clone(),
                dir: dir.to_path_buf(),
            }))
            .unwrap();

        std::thread::scope(|scope| {
            for _ in 0..worker_count {
                let queue_tx = queue_tx.clone();
                let queue_rx = queue_rx.clone();
                let pending = &pending;
                let cancelled = &cancelled;
                scope.spawn(move || {
                    while let Ok(message) = queue_rx.recv() {
                        let QueueMessage::Directory(mut task) = message else {
                            break;
                        };

                        let children = if cancelled.load(Ordering::Relaxed) {
                            Vec::new()
                        } else {
                            match task.walker.walk_directory(&task.dir) {
                                Some(children) => children,
                                None => {
                                    cancelled.store(true, Ordering::Relaxed);
                                    Vec::new()
                                }
                            }
                        };

                        pending.fetch_add(children.len(), Ordering::Relaxed);
                        for child in children {
                            let _ = queue_tx.send(QueueMessage::Directory(child));
                        }

                        if pending.fetch_sub(1, Ordering::AcqRel) == 1 {
                            for _ in 0..worker_count {
                                let _ = queue_tx.send(QueueMessage::Stop);
                            }
                        }
                    }
                });
            }
        });

        !cancelled.load(Ordering::Relaxed)
    }

    fn walk_root_file(&mut self, path: &Path) -> bool {
        let metadata = match fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => return true,
            Err(_error) => {
                diag_warn!("{}: {_error}", path.display());
                return true;
            }
        };
        {
            let mut state = self.state.lock().unwrap();
            state.count += 1;
            state.path = path.to_path_buf();
        }
        self.send_file(path.to_path_buf(), metadata.len() as usize)
    }

    fn load_initial_rules(&mut self, root: &Path) {
        let mut parents = root.ancestors().skip(1).collect::<Vec<_>>();
        parents.reverse();

        let git_root = self.repository.as_ref().map(|repo| repo.root.as_path());
        for parent in parents {
            if !self.args.include_ignored {
                self.rules.ignore.add_file(&parent.join(".ignore"), parent);
            }
            if !self.args.include_gitignored
                && git_root.is_some_and(|git_root| parent.starts_with(git_root))
            {
                self.rules
                    .gitignore
                    .add_file(&parent.join(".gitignore"), parent);
            }
        }

        self.load_repository_rules();
    }

    fn load_repository_rules(&mut self) {
        let Some(repository) = &self.repository else {
            return;
        };
        if !self.args.include_gitexcluded {
            self.rules
                .git_exclude
                .add_file(&repository.git_dir.join("info/exclude"), &repository.root);
        }
        if !self.args.include_all
            && let Some(path) = global_ignore_path()
        {
            self.rules.git_global.add_file(&path, &repository.root);
        }
    }

    fn walk_directory(&mut self, dir: &Path) -> Option<Vec<DirectoryTask>> {
        self.enter_repository(dir);
        self.load_directory_rules(dir);

        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_error) => {
                diag_warn!("{}: {_error}", dir.display());
                return Some(Vec::new());
            }
        };
        let mut directories = Vec::new();
        for result in entries {
            let entry = match result {
                Ok(entry) => entry,
                Err(_error) => {
                    diag_warn!("{}: {_error}", dir.display());
                    continue;
                }
            };
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_error) => {
                    diag_warn!("{}: {_error}", path.display());
                    continue;
                }
            };
            if !file_type.is_dir() && !file_type.is_file() {
                continue;
            }
            match self.walk_task(WalkTask { path, file_type }) {
                TaskResult::Continue => {}
                TaskResult::Directory(dir) => directories.push(DirectoryTask {
                    walker: self.clone(),
                    dir,
                }),
                TaskResult::Cancel => return None,
            }
        }
        Some(directories)
    }

    fn load_directory_rules(&mut self, dir: &Path) {
        if !self.args.include_ignored {
            self.rules.ignore.add_file(&dir.join(".ignore"), dir);
        }
        if self.repository.is_some() && !self.args.include_gitignored {
            self.rules.gitignore.add_file(&dir.join(".gitignore"), dir);
        }
    }

    fn walk_task(&mut self, task: WalkTask) -> TaskResult {
        let WalkTask { path, file_type } = task;
        let is_dir = file_type.is_dir();
        let is_file = file_type.is_file();
        let override_decision = self.overrides.matched(&path, is_dir);
        if override_decision == Some(Decision::Exclude) {
            return TaskResult::Continue;
        }
        let explicitly_included = override_decision == Some(Decision::Include);
        if !explicitly_included {
            if self.overrides.has_includes && is_file {
                return TaskResult::Continue;
            }
            let ignore_decision = self.rules.matched(&path, is_dir, self.repository.is_some());
            if ignore_decision == Some(Decision::Exclude) {
                return TaskResult::Continue;
            }
            if ignore_decision.is_none()
                && !self.args.include_hidden
                && path.file_name().is_some_and(is_hidden)
            {
                return TaskResult::Continue;
            }
        }

        {
            let mut state = self.state.lock().unwrap();
            state.count += 1;
            state.path = path.clone();
        }

        if is_dir {
            if !self.args.cross_fs
                && self
                    .root_device
                    .zip(device_id(&path))
                    .is_some_and(|(root, child)| root != child)
            {
                return TaskResult::Continue;
            }
            return TaskResult::Directory(path);
        }

        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(_error) => {
                diag_warn!("{}: {_error}", path.display());
                return TaskResult::Continue;
            }
        };
        if self.send_file(path, metadata.len() as usize) {
            TaskResult::Continue
        } else {
            TaskResult::Cancel
        }
    }

    fn send_file(&mut self, path: PathBuf, size: usize) -> bool {
        if size == 0 {
            return true;
        }
        {
            let mut state = self.state.lock().unwrap();
            state.total += size;
        }
        let color = self.colors.file_color(&path);
        self.tx
            .send(Entry {
                path,
                size,
                nfiles: 1,
                leaves: 1,
                color,
                ..Default::default()
            })
            .is_ok()
    }

    fn enter_repository(&mut self, dir: &Path) {
        if self
            .repository
            .as_ref()
            .is_some_and(|repository| repository.root == dir)
        {
            return;
        }
        let Some(repository) = repository_at(dir) else {
            return;
        };
        self.repository = Some(repository);
        self.rules.gitignore = RuleSet::default();
        self.rules.git_exclude = RuleSet::default();
        self.rules.git_global = RuleSet::default();
        self.load_repository_rules();
    }
}

enum TaskResult {
    Continue,
    Directory(PathBuf),
    Cancel,
}

fn parse_rule(line: &str, base: &Path, override_rule: bool) -> Option<Rule> {
    #[cfg(not(test))]
    let _ = base;
    let mut pattern = trim_unescaped_spaces(line.trim_end_matches('\r'));
    if pattern.is_empty() || pattern.starts_with('#') {
        return None;
    }
    let mut negated = false;
    if let Some(value) = pattern.strip_prefix('!') {
        negated = true;
        pattern = value;
    }
    if pattern.is_empty() {
        return None;
    }

    let dir_only = pattern.ends_with('/') && !pattern.ends_with("\\/");
    if dir_only {
        pattern = &pattern[..pattern.len() - 1];
    }
    let anchored = pattern.starts_with('/');
    if let Some(value) = pattern.strip_prefix('/') {
        pattern = value;
    }
    if pattern.is_empty() {
        return None;
    }

    let pattern = if pattern.starts_with('!') {
        format!("\\{pattern}")
    } else {
        pattern.to_string()
    };
    let basename_only = !anchored && !pattern.contains('/');
    let decision = if override_rule {
        if negated {
            Decision::Exclude
        } else {
            Decision::Include
        }
    } else if negated {
        Decision::Include
    } else {
        Decision::Exclude
    };

    Some(Rule {
        #[cfg(test)]
        base: base.to_path_buf(),
        pattern,
        decision,
        dir_only,
        basename_only,
    })
}

fn trim_unescaped_spaces(mut line: &str) -> &str {
    while let Some(value) = line.strip_suffix(' ') {
        let backslashes = value
            .bytes()
            .rev()
            .take_while(|byte| *byte == b'\\')
            .count();
        if backslashes % 2 == 1 {
            break;
        }
        line = value;
    }
    line
}

fn validate_override(pattern: &str) -> Result<()> {
    let mut escaped = false;
    let mut brackets = 0usize;
    let mut braces = 0usize;
    for byte in pattern.bytes() {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' => escaped = true,
            b'[' => brackets += 1,
            b']' if brackets > 0 => brackets -= 1,
            b'{' => braces += 1,
            b'}' if braces > 0 => braces -= 1,
            _ => {}
        }
    }
    if brackets == 0 && braces == 0 && !escaped {
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!("invalid override glob {pattern:?}"),
    )
    .into())
}

fn slash_path(path: &Path) -> Cow<'_, str> {
    let path = path.to_string_lossy();
    if path.contains('\\') {
        Cow::Owned(path.replace('\\', "/"))
    } else {
        path
    }
}

fn is_hidden(name: &OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

fn find_repository(path: &Path) -> Option<Repository> {
    path.ancestors().find_map(repository_at)
}

fn repository_at(path: &Path) -> Option<Repository> {
    let marker = path.join(".git");
    if marker.is_dir() {
        return Some(Repository {
            root: path.to_path_buf(),
            git_dir: marker,
        });
    }
    let text = fs::read_to_string(marker).ok()?;
    let git_dir = text.trim().strip_prefix("gitdir:")?.trim();
    let git_dir = Path::new(git_dir);
    Some(Repository {
        root: path.to_path_buf(),
        git_dir: if git_dir.is_absolute() {
            git_dir.to_path_buf()
        } else {
            path.join(git_dir)
        },
    })
}

fn global_ignore_path() -> Option<PathBuf> {
    let home_dir = dirs::home_dir();
    let config_dir = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir.as_ref().map(|home| home.join(".config")));

    let configured = home_dir
        .as_ref()
        .map(|home| home.join(".gitconfig"))
        .into_iter()
        .chain(config_dir.as_ref().map(|config| config.join("git/config")))
        .find_map(|path| configured_excludes_file(&path, home_dir.as_deref()));
    configured.or_else(|| config_dir.map(|config| config.join("git/ignore")))
}

fn configured_excludes_file(path: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let text = fs::read_to_string(path).ok()?;
    let mut in_core = false;
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.starts_with('[') {
            in_core = line.eq_ignore_ascii_case("[core]");
            continue;
        }
        if !in_core || matches!(line.as_bytes().first(), Some(b'#' | b';')) {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("excludesfile") {
            continue;
        }
        let value = value.trim().trim_matches('"');
        if let Some(relative) = value.strip_prefix("~/") {
            return home.map(|home| home.join(relative));
        }
        return Some(PathBuf::from(value));
    }
    None
}

#[cfg(unix)]
fn device_id(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).ok().map(|metadata| metadata.dev())
}

#[cfg(not(unix))]
fn device_id(_path: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::config::Config;
    use ignore::gitignore::GitignoreBuilder;
    use ignore::overrides::OverrideBuilder;

    fn expected_decision(patterns: &[&str], relative: &str, is_dir: bool) -> Option<Decision> {
        let base = Path::new("/repo");
        let mut builder = GitignoreBuilder::new(base);
        for pattern in patterns {
            builder.add_line(None, pattern).unwrap();
        }
        let matcher = builder.build().unwrap();
        let matched = matcher.matched(base.join(relative), is_dir);
        if matched.is_ignore() {
            Some(Decision::Exclude)
        } else if matched.is_whitelist() {
            Some(Decision::Include)
        } else {
            None
        }
    }

    fn actual_decision(patterns: &[&str], relative: &str, is_dir: bool) -> Option<Decision> {
        let base = Path::new("/repo");
        let parsed = patterns
            .iter()
            .filter_map(|pattern| parse_rule(pattern, base, false))
            .collect::<Vec<_>>();
        let mut rules = RuleSet::default();
        rules.add_rules(base, &parsed).unwrap();
        rules.matched(&base.join(relative), is_dir)
    }

    fn assert_compatible(patterns: &[&str], candidates: &[(&str, bool)]) {
        for &(relative, is_dir) in candidates {
            assert_eq!(
                actual_decision(patterns, relative, is_dir),
                expected_decision(patterns, relative, is_dir),
                "patterns {patterns:?}, candidate {relative:?}, is_dir {is_dir}"
            );
        }
    }

    fn make_args(path: &Path, overrides: &[&str]) -> Args {
        Args {
            path: path.to_path_buf(),
            max_depth: 5,
            cross_fs: false,
            xray: false,
            include_all: false,
            include_hidden: false,
            include_ignored: false,
            include_gitignored: false,
            include_gitexcluded: false,
            overrides: overrides.iter().map(|value| value.to_string()).collect(),
        }
    }

    fn nano_paths(root: &Path, args: &Args) -> BTreeSet<String> {
        let colors = ColorScheme::new(&Config::default());
        spawn_walker(&colors, args, Default::default(), root)
            .unwrap()
            .into_iter()
            .map(|entry| slash_path(entry.path.strip_prefix(root).unwrap()).into_owned())
            .collect()
    }

    fn existing_paths(root: &Path, args: &Args) -> BTreeSet<String> {
        let mut overrides = OverrideBuilder::new(&args.path);
        for pattern in &args.overrides {
            overrides.add(pattern).unwrap();
        }
        ignore::WalkBuilder::new(root)
            .overrides(overrides.build().unwrap())
            .hidden(!args.include_hidden)
            .ignore(!args.include_ignored)
            .git_ignore(!args.include_gitignored)
            .git_exclude(!args.include_gitexcluded)
            .same_file_system(!args.cross_fs)
            .build()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_type()
                    .is_some_and(|file_type| file_type.is_file())
            })
            .filter(|entry| entry.metadata().is_ok_and(|metadata| metadata.len() > 0))
            .map(|entry| slash_path(entry.path().strip_prefix(root).unwrap()).into_owned())
            .collect()
    }

    #[test]
    fn parses_gitignore_and_override_polarity() {
        let base = Path::new("/repo");
        let ignore = parse_rule("target/", base, false).unwrap();
        assert_eq!(ignore.decision, Decision::Exclude);
        assert!(ignore.dir_only);
        assert!(ignore.matches(Path::new("/repo/crate/target"), true));

        let include = parse_rule("!target/keep", base, false).unwrap();
        assert_eq!(include.decision, Decision::Include);
        assert!(include.matches(Path::new("/repo/target/keep"), false));

        let override_exclude = parse_rule("!*.log", base, true).unwrap();
        assert_eq!(override_exclude.decision, Decision::Exclude);
        assert!(override_exclude.matches(Path::new("/repo/deep/debug.log"), false));
    }

    #[test]
    fn anchored_rules_are_relative_to_their_ignore_file() {
        let base = Path::new("/repo/crate");
        let anchored = parse_rule("/build/*.o", base, false).unwrap();
        assert!(anchored.matches(Path::new("/repo/crate/build/main.o"), false));
        assert!(!anchored.matches(Path::new("/repo/crate/src/build/main.o"), false));

        let basename = parse_rule("*.o", base, false).unwrap();
        assert!(basename.matches(Path::new("/repo/crate/src/main.o"), false));
    }

    #[test]
    fn later_and_higher_precedence_rules_win() {
        let base = Path::new("/repo");
        let mut rules = Rules::default();
        rules
            .gitignore
            .add_rules(
                base,
                &[
                    parse_rule("*.tmp", base, false).unwrap(),
                    parse_rule("!keep.tmp", base, false).unwrap(),
                ],
            )
            .unwrap();
        rules
            .ignore
            .add_rules(base, &[parse_rule("keep.tmp", base, false).unwrap()])
            .unwrap();

        assert_eq!(
            rules.matched(Path::new("/repo/drop.tmp"), false, true),
            Some(Decision::Exclude)
        );
        assert_eq!(
            rules.matched(Path::new("/repo/keep.tmp"), false, true),
            Some(Decision::Exclude)
        );

        assert_compatible(
            &["duplicate", "!duplicate"],
            &[("duplicate", false), ("other", false)],
        );
    }

    #[test]
    fn matches_common_gitignore_semantics() {
        assert_compatible(
            &["*.log", "!keep.log"],
            &[
                ("debug.log", false),
                ("deep/debug.log", false),
                ("keep.log", false),
                ("deep/keep.log", false),
                ("debug.txt", false),
            ],
        );
        assert_compatible(
            &["/build/*.o"],
            &[
                ("build/main.o", false),
                ("deep/build/main.o", false),
                ("build/deep/main.o", false),
            ],
        );
        assert_compatible(
            &["doc/frotz", "abc/**", "a/**/b"],
            &[
                ("doc/frotz", false),
                ("deep/doc/frotz", false),
                ("abc/file", false),
                ("abc/deep/file", false),
                ("a/b", false),
                ("a/deep/b", false),
                ("a/deep/more/b", false),
            ],
        );
        assert_compatible(
            &["cache/", "[ab].txt", "\\#notes", "\\!important"],
            &[
                ("cache", true),
                ("cache", false),
                ("deep/cache", true),
                ("a.txt", false),
                ("c.txt", false),
                ("#notes", false),
                ("!important", false),
            ],
        );
        assert_compatible(
            &["foo", "**/bar", "a?c", "[!a].txt", "name\\ "],
            &[
                ("foo", false),
                ("deep/foo", false),
                ("foobar", false),
                ("bar", false),
                ("deep/bar", false),
                ("abc", false),
                ("a/c", false),
                ("b.txt", false),
                ("a.txt", false),
                ("name ", false),
                ("name", false),
            ],
        );
        assert_compatible(
            &["foo/**", "src/*/generated"],
            &[
                ("foo", true),
                ("foo/file", false),
                ("foo/deep/file", false),
                ("deep/foo/file", false),
                ("src/a/generated", false),
                ("src/a/deep/generated", false),
            ],
        );
    }

    #[test]
    fn matches_existing_override_semantics() {
        let base = Path::new("/repo");
        let patterns = ["*.foo", "!*.bar.foo"];
        let ours = Overrides::new(&make_args(base, &patterns)).unwrap();
        let mut builder = OverrideBuilder::new(base);
        for pattern in patterns {
            builder.add(pattern).unwrap();
        }
        let existing = builder.build().unwrap();

        for (relative, is_dir) in [
            ("a.foo", false),
            ("a.rs", false),
            ("a.bar.foo", false),
            ("src/a.foo", false),
            ("src", true),
        ] {
            let path = base.join(relative);
            let expected = existing.matched(&path, is_dir);
            let expected = if expected.is_ignore() {
                Some(Decision::Exclude)
            } else if expected.is_whitelist() {
                Some(Decision::Include)
            } else {
                None
            };
            let actual = ours
                .matched(&path, is_dir)
                .or_else(|| (ours.has_includes && !is_dir).then_some(Decision::Exclude));
            assert_eq!(actual, expected, "{relative:?}, is_dir {is_dir}");
        }
    }

    #[test]
    fn walker_applies_all_ignore_sources_and_precedence() {
        let fixture = Fixture::new();
        fixture.dir(".git/info");
        fixture.dir("nested");
        fixture.dir("ignored");
        fixture.file(".git/info/exclude", "excluded.bin\n");
        fixture.file(".gitignore", "*.log\n!keep.log\nignored/\n");
        fixture.file(".ignore", "!debug.log\n*.tmp\n");
        fixture.file("keep.txt", "x");
        fixture.file("debug.log", "x");
        fixture.file("keep.log", "x");
        fixture.file("scratch.tmp", "x");
        fixture.file("excluded.bin", "x");
        fixture.file(".hidden", "x");
        fixture.file("ignored/data.txt", "x");
        fixture.file("nested/.gitignore", "*.bin\n");
        fixture.file("nested/drop.bin", "x");
        fixture.file("nested/keep.txt", "x");

        let root = fixture.path.canonicalize().unwrap();
        let args = make_args(&root, &[]);
        let entries = nano_paths(&root, &args);
        let existing = existing_paths(&root, &args);

        assert_eq!(
            entries,
            BTreeSet::from([
                "debug.log".to_string(),
                "keep.log".to_string(),
                "keep.txt".to_string(),
                "nested/keep.txt".to_string(),
            ])
        );
        assert_eq!(entries, existing);

        for configure in [
            |args: &mut Args| args.include_hidden = true,
            |args: &mut Args| args.include_ignored = true,
            |args: &mut Args| args.include_gitignored = true,
            |args: &mut Args| args.include_gitexcluded = true,
        ] {
            let mut args = make_args(&root, &[]);
            configure(&mut args);
            assert_eq!(
                nano_paths(&root, &args),
                existing_paths(&root, &args),
                "filter configuration {args:?}"
            );
        }

        let args = make_args(&root, &["*.txt", "!ignored/**"]);
        assert_eq!(
            nano_paths(&root, &args),
            existing_paths(&root, &args),
            "explicit overrides"
        );
    }

    struct Fixture {
        path: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "leaves-nano-scan-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self { path }
        }

        fn dir(&self, relative: &str) {
            fs::create_dir_all(self.path.join(relative)).unwrap();
        }

        fn file(&self, relative: &str, contents: &str) {
            fs::write(self.path.join(relative), contents).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.path).unwrap();
        }
    }
}
