use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const MAX_GIT_STDOUT_BYTES: usize = 512 * 1024;
const MAX_GIT_STDERR_BYTES: usize = 16 * 1024;
const COMMIT_PAGE_SIZE: usize = 500;
const MAX_GRAPH_LANES: usize = 4096;
/// Reference listings grow with the repository (about 100 bytes per ref), so they get a larger cap than diffs.
const MAX_HISTORY_REFS_BYTES: usize = 16 * 1024 * 1024;
const GIT_TIMEOUT: Duration = Duration::from_secs(120);
/// Commits may run slow hooks or wait for a signing prompt.
const GIT_COMMIT_TIMEOUT: Duration = Duration::from_secs(600);

/// Git command that never creates a console window (the release build uses the GUI subsystem, so each spawn would otherwise allocate a console host).
fn git_command() -> Command {
    let mut command = Command::new("git");
    // A repository can set core.fsmonitor to a command that Git then runs on every status; never honor it.
    command.args(["-c", "core.fsmonitor=false"]);
    command.env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, 0x0800_0000); // CREATE_NO_WINDOW
    command
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReferenceKind {
    Local,
    Remote,
    Tag,
}

#[derive(Clone, Deserialize, Serialize)]
struct GitReference {
    id: String,
    name: String,
    kind: ReferenceKind,
}

#[derive(Clone, Deserialize, Serialize)]
struct HistoryRefs {
    tips: Vec<String>,
    references: Vec<GitReference>,
}

#[derive(Serialize)]
struct Commit {
    id: String,
    subject: String,
    author: String,
    date: String,
    parents: Vec<String>,
    lane: usize,
    passing_lanes: Vec<usize>,
    merging_lanes: Vec<usize>,
    forking_off_lanes: Vec<usize>,
    child_count: usize,
    references: Vec<GitReference>,
}

#[derive(Clone, Default, serde::Deserialize, Serialize)]
struct GraphState {
    reserved_lanes: BTreeMap<usize, String>,
}

#[derive(Serialize)]
struct RepositorySummary {
    path: String,
    name: String,
    branch: String,
    history_head: Option<String>,
    history_refs: HistoryRefs,
    status: RepositoryStatus,
    commit: CommitMetadata,
    commits: Vec<Commit>,
    has_more_commits: bool,
    graph_state: GraphState,
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct CommitIdentity {
    name: Option<String>,
    email: Option<String>,
}

#[derive(Serialize)]
struct CommitMetadata {
    draft: String,
    previous_message: Option<String>,
    local_identity: CommitIdentity,
    global_identity: CommitIdentity,
}

#[derive(Default, Serialize)]
struct RepositoryStatus {
    staged: Vec<StatusEntry>,
    unstaged: Vec<StatusEntry>,
    truncated: bool,
    /// A merge is waiting for its commit; it can be committed with an empty index.
    merging: bool,
}

#[derive(Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum StatusGroup {
    Staged,
    Unstaged,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ConflictSide {
    Current,
    Incoming,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum FileDiffKind {
    Text,
    Binary,
    Submodule,
    Conflict,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
enum ConflictVersionKind {
    Text,
    Binary,
    Submodule,
    Deleted,
    TooLarge,
}

#[derive(Serialize)]
struct ConflictVersion {
    kind: ConflictVersionKind,
    content: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConflictStage {
    mode: Vec<u8>,
    object_id: Vec<u8>,
}

#[derive(Serialize)]
struct FileDiff {
    kind: FileDiffKind,
    patch: Option<String>,
    current: Option<ConflictVersion>,
    incoming: Option<ConflictVersion>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct StatusEntry {
    path: String,
    path_bytes: Vec<u8>,
    kind: StatusKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum StatusKind {
    Added,
    Modified,
    Removed,
    Conflicting,
}

#[derive(Serialize)]
struct CommitPage {
    commits: Vec<Commit>,
    has_more_commits: bool,
    graph_state: GraphState,
}

fn read_capped(mut reader: impl Read, limit: usize) -> io::Result<(Vec<u8>, bool)> {
    read_capped_signaling(&mut reader, limit, None)
}

fn read_capped_signaling(
    mut reader: impl Read,
    limit: usize,
    overflow: Option<Arc<AtomicBool>>,
) -> io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::with_capacity(limit.min(8192));
    let mut truncated = false;
    let mut chunk = [0; 8192];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(bytes.len());
        bytes.extend_from_slice(&chunk[..read.min(remaining)]);
        if read > remaining {
            truncated = true;
            if let Some(overflow) = &overflow {
                overflow.store(true, Ordering::Relaxed);
            }
        }
    }
    Ok((bytes, truncated))
}

fn collect_output(mut child: Child) -> Result<Output, String> {
    collect_output_with_input(&mut child, None)
}

fn collect_output_with_input(child: &mut Child, input: Option<&[u8]>) -> Result<Output, String> {
    collect_output_inner(
        child,
        input,
        false,
        false,
        MAX_GIT_STDOUT_BYTES,
        GIT_TIMEOUT,
    )
    .map(|(output, _)| output)
}

fn collect_output_with_truncated_stdout(child: Child) -> Result<(Output, bool), String> {
    let mut child = child;
    collect_output_inner(
        &mut child,
        None,
        true,
        false,
        MAX_GIT_STDOUT_BYTES,
        GIT_TIMEOUT,
    )
}

fn collect_output_killing_on_truncation(child: Child) -> Result<Output, String> {
    let mut child = child;
    collect_output_inner(
        &mut child,
        None,
        false,
        true,
        MAX_GIT_STDOUT_BYTES,
        GIT_TIMEOUT,
    )
    .map(|(output, _)| output)
}

fn collect_output_capped_and_killing_on_truncation(child: Child) -> Result<(Output, bool), String> {
    let mut child = child;
    collect_output_inner(
        &mut child,
        None,
        true,
        true,
        MAX_GIT_STDOUT_BYTES,
        GIT_TIMEOUT,
    )
}

fn collect_output_inner(
    child: &mut Child,
    input: Option<&[u8]>,
    allow_truncated_stdout: bool,
    kill_on_truncated_stdout: bool,
    stdout_limit: usize,
    timeout: Duration,
) -> Result<(Output, bool), String> {
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let stdout_overflow = Arc::new(AtomicBool::new(false));
    let stdout_reader_overflow = kill_on_truncated_stdout.then(|| Arc::clone(&stdout_overflow));
    let stdout_reader =
        thread::spawn(move || read_capped_signaling(stdout, stdout_limit, stdout_reader_overflow));
    let stderr_reader = thread::spawn(move || read_capped(stderr, MAX_GIT_STDERR_BYTES));
    if let Some(input) = input {
        let result = child.stdin.take().expect("piped stdin").write_all(input);
        if let Err(error) = result {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(format!("Could not write Git input: {error}"));
        }
    }
    // Poll instead of blocking in wait() so a hung Git (credential or signing prompt, stuck hook) can be killed.
    let deadline = Instant::now() + timeout;
    let mut delay = Duration::from_micros(250);
    let status = loop {
        if stdout_overflow.load(Ordering::Relaxed) {
            let _ = child.kill();
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                // The readers are left to finish on their own: a hook that outlived Git may still hold the pipes.
                return Err(format!(
                    "Git did not finish within {} seconds and was stopped",
                    timeout.as_secs()
                ));
            }
            Ok(None) => {
                thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_millis(2));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("Could not wait for Git: {error}"));
            }
        }
    };
    let (stdout, stdout_truncated) = stdout_reader
        .join()
        .map_err(|_| "Could not read Git output".to_owned())?
        .map_err(|error| format!("Could not read Git output: {error}"))?;
    let (mut stderr, stderr_truncated) = stderr_reader
        .join()
        .map_err(|_| "Could not read Git error output".to_owned())?
        .map_err(|error| format!("Could not read Git error output: {error}"))?;

    if stdout_truncated && !allow_truncated_stdout {
        return Err(format!(
            "Git output exceeded the {} KiB limit",
            stdout_limit / 1024
        ));
    }
    if stderr_truncated {
        stderr.extend_from_slice(b"\n[Git error output truncated]");
    }
    Ok((
        Output {
            status,
            stdout,
            stderr,
        },
        stdout_truncated,
    ))
}

fn git_output(path: &Path, args: &[&str], locale: Option<&str>) -> Result<Output, String> {
    let mut command = git_command();
    command
        .arg("-C")
        .arg(path)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(locale) = locale {
        command.env("LC_ALL", locale);
    }
    let child = command
        .spawn()
        .map_err(|error| format!("Could not start Git: {error}"))?;
    collect_output(child)
}

fn git_child_with_paths(path: &Path, args: &[&str], paths: &[&OsStr]) -> Result<Child, String> {
    let mut command = git_command();
    command
        .arg("-C")
        .arg(path)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.args(paths);
    command
        .spawn()
        .map_err(|error| format!("Could not start Git: {error}"))
}

fn git_output_with_paths(
    path: &Path,
    args: &[&str],
    paths: &[&OsStr],
    kill_on_truncated_stdout: bool,
) -> Result<Output, String> {
    let child = git_child_with_paths(path, args, paths)?;
    if kill_on_truncated_stdout {
        collect_output_killing_on_truncation(child)
    } else {
        collect_output(child)
    }
}

fn git_output_with_paths_capped(
    path: &Path,
    args: &[&str],
    paths: &[&OsStr],
) -> Result<(Output, bool), String> {
    let child = git_child_with_paths(path, args, paths)?;
    collect_output_capped_and_killing_on_truncation(child)
}

fn git_output_with_input(path: &Path, args: &[&str], input: &[u8]) -> Result<Output, String> {
    let mut command = git_command();
    command
        .arg("-C")
        .arg(path)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start Git: {error}"))?;
    collect_output_with_input(&mut child, Some(input))
}

fn git(path: &Path, args: &[&str]) -> Result<Output, String> {
    let output = git_output(path, args, None)?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(output)
}

fn git_text(path: &Path, args: &[&str]) -> Result<String, String> {
    git(path, args).map(|output| {
        String::from_utf8_lossy(&output.stdout)
            .trim_end_matches(['\r', '\n'])
            .to_owned()
    })
}

fn git_text_with_stdout_limit(
    path: &Path,
    args: &[&str],
    stdout_limit: usize,
) -> Result<String, String> {
    let mut command = git_command();
    command
        .arg("-C")
        .arg(path)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start Git: {error}"))?;
    let (output, _) =
        collect_output_inner(&mut child, None, false, false, stdout_limit, GIT_TIMEOUT)?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end_matches(['\r', '\n'])
        .to_owned())
}

fn git_text_with_input(path: &Path, args: &[&str], input: &[u8]) -> Result<String, String> {
    let output = git_output_with_input(path, args, input)?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end_matches(['\r', '\n'])
        .to_owned())
}

fn parse_commits(output: &str) -> Vec<Commit> {
    let fields: Vec<_> = output.split('\0').collect();
    fields
        .as_chunks::<5>()
        .0
        .iter()
        .filter(|fields| !fields[0].trim_matches(['\r', '\n']).is_empty())
        .map(|fields| Commit {
            id: fields[0].trim_matches(['\r', '\n']).to_owned(),
            parents: fields[1].split_whitespace().map(str::to_owned).collect(),
            subject: fields[2].trim_end().to_owned(),
            author: fields[3].trim_end().to_owned(),
            date: fields[4].trim_end_matches(['\r', '\n']).to_owned(),
            lane: 0,
            passing_lanes: Vec::new(),
            merging_lanes: Vec::new(),
            forking_off_lanes: Vec::new(),
            child_count: 0,
            references: Vec::new(),
        })
        .collect()
}

fn parse_status(output: &[u8], truncated: bool) -> Result<RepositoryStatus, String> {
    let output = if truncated {
        output
            .iter()
            .rposition(|byte| *byte == 0)
            .map_or(&[][..], |end| &output[..=end])
    } else {
        output
    };
    let mut status = RepositoryStatus {
        truncated,
        ..RepositoryStatus::default()
    };
    let mut records = output.split(|byte| *byte == 0);
    while let Some(record) = records.next().filter(|record| !record.is_empty()) {
        let record_type = record[0];
        if record_type == b'#' || record_type == b'!' {
            continue;
        }
        if record_type == b'?' {
            let path = record
                .strip_prefix(b"? ")
                .ok_or_else(|| "Malformed Git status record".to_owned())?;
            status.unstaged.push(status_entry(path, StatusKind::Added));
            continue;
        }

        let field_count = match record_type {
            b'1' => 9,
            b'2' => 10,
            b'u' => 11,
            _ => return Err("Unsupported Git status record".to_owned()),
        };
        let fields = record
            .splitn(field_count, |byte| *byte == b' ')
            .collect::<Vec<_>>();
        if fields.len() != field_count
            || fields[1].len() != 2
            || fields.last().is_none_or(|p| p.is_empty())
        {
            return Err("Malformed Git status record".to_owned());
        }
        let original_path = if record_type == b'2' {
            match records.next().filter(|path| !path.is_empty()) {
                Some(path) => Some(path),
                None if truncated => break,
                None => return Err("Malformed Git rename record".to_owned()),
            }
        } else {
            None
        };

        let pair = fields[1];
        if record_type == b'u' || pair.contains(&b'U') {
            status.unstaged.push(status_entry(
                fields[field_count - 1],
                StatusKind::Conflicting,
            ));
            continue;
        }
        add_status_entry(
            &mut status.staged,
            pair[0],
            fields[field_count - 1],
            original_path,
        )?;
        add_status_entry(
            &mut status.unstaged,
            pair[1],
            fields[field_count - 1],
            original_path,
        )?;
    }
    status
        .staged
        .sort_by(|left, right| left.path_bytes.cmp(&right.path_bytes));
    status
        .unstaged
        .sort_by(|left, right| left.path_bytes.cmp(&right.path_bytes));
    Ok(status)
}

fn add_status_entry(
    entries: &mut Vec<StatusEntry>,
    code: u8,
    path: &[u8],
    original_path: Option<&[u8]>,
) -> Result<(), String> {
    if code == b'.' {
        return Ok(());
    }
    if code == b'R' {
        let original_path =
            original_path.ok_or_else(|| "Malformed Git rename record".to_owned())?;
        entries.push(status_entry(path, StatusKind::Added));
        entries.push(status_entry(original_path, StatusKind::Removed));
    } else {
        entries.push(status_entry(path, status_kind(code)));
    }
    Ok(())
}

fn status_entry(path: &[u8], kind: StatusKind) -> StatusEntry {
    StatusEntry {
        path: display_git_path(path),
        path_bytes: path.to_vec(),
        kind,
    }
}

fn display_git_path(path: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut result = String::new();
    let mut remaining = path;
    while let Err(error) = std::str::from_utf8(remaining) {
        let valid = error.valid_up_to();
        append_display_text(
            &mut result,
            std::str::from_utf8(&remaining[..valid]).unwrap(),
        );
        let invalid = error
            .error_len()
            .unwrap_or_else(|| remaining.len().saturating_sub(valid));
        for byte in &remaining[valid..valid + invalid] {
            write!(result, "\\x{byte:02X}").unwrap();
        }
        remaining = &remaining[valid + invalid..];
    }
    append_display_text(&mut result, std::str::from_utf8(remaining).unwrap());
    result
}

fn append_display_text(output: &mut String, text: &str) {
    use std::fmt::Write as _;

    for character in text.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                write!(output, "\\u{{{:X}}}", character as u32).unwrap();
            }
            character => output.push(character),
        }
    }
}

fn status_kind(code: u8) -> StatusKind {
    match code {
        b'A' | b'C' | b'?' => StatusKind::Added,
        b'D' => StatusKind::Removed,
        _ => StatusKind::Modified,
    }
}

fn repository_status(path: &Path) -> Result<RepositoryStatus, String> {
    let mut command = git_command();
    command
        .arg("-C")
        .arg(path)
        .args(["status", "--porcelain=v2", "-z", "--untracked-files=all"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command
        .spawn()
        .map_err(|error| format!("Could not start Git: {error}"))?;
    let (output, truncated) = collect_output_with_truncated_stdout(child)?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let mut status = parse_status(&output.stdout, truncated)?;
    status.merging = merge_in_progress(path)?;
    Ok(status)
}

fn merge_in_progress(path: &Path) -> Result<bool, String> {
    // Plain repositories keep MERGE_HEAD in `.git`; skip a Git spawn on the hot status path.
    let dot_git = path.join(".git");
    if dot_git.is_dir() {
        return Ok(dot_git.join("MERGE_HEAD").exists());
    }
    Ok(git_path(path, "MERGE_HEAD")?.exists())
}

fn git_path_os_string(path_bytes: &[u8]) -> Result<OsString, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(OsString::from_vec(path_bytes.to_vec()))
    }
    #[cfg(windows)]
    {
        Ok(OsString::from(
            String::from_utf8(path_bytes.to_vec())
                .map_err(|_| "Git path cannot be represented on Windows".to_owned())?,
        ))
    }
}

fn is_gitlink(path: &Path, path_bytes: &[u8]) -> Result<bool, String> {
    let path_argument = git_path_os_string(path_bytes)?;
    let args = ["--literal-pathspecs", "ls-files", "--stage", "-z", "--"];
    let output = git_output_with_paths(path, &args, &[path_argument.as_os_str()], false)?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(output.stdout.split(|byte| *byte == 0).any(|record| {
        record
            .iter()
            .position(|byte| *byte == b'\t')
            .is_some_and(|separator| {
                record[..separator].starts_with(b"160000 ")
                    && &record[separator + 1..] == path_bytes
            })
    }))
}

fn conflict_stages(path: &Path, path_bytes: &[u8]) -> Result<BTreeMap<u8, ConflictStage>, String> {
    let path_argument = git_path_os_string(path_bytes)?;
    let args = ["--literal-pathspecs", "ls-files", "--stage", "-z", "--"];
    let index = git_output_with_paths(path, &args, &[path_argument.as_os_str()], false)?;
    if !index.status.success() {
        return Err(String::from_utf8_lossy(&index.stderr).trim().to_owned());
    }

    let mut stages = BTreeMap::new();
    for record in index.stdout.split(|byte| *byte == 0) {
        let Some(separator) = record.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        if &record[separator + 1..] != path_bytes {
            continue;
        }
        let fields: Vec<_> = record[..separator]
            .split(|byte| *byte == b' ')
            .filter(|field| !field.is_empty())
            .collect();
        if fields.len() != 3 {
            return Err("Git returned an invalid conflict index entry".to_owned());
        }
        let stage = match fields[2] {
            b"0" => 0,
            b"1" => 1,
            b"2" => 2,
            b"3" => 3,
            _ => return Err("Git returned an invalid index stage".to_owned()),
        };
        if !matches!(fields[1].len(), 40 | 64) || !fields[1].iter().all(u8::is_ascii_hexdigit) {
            return Err("Git returned an invalid conflict object ID".to_owned());
        }
        if stages
            .insert(
                stage,
                ConflictStage {
                    mode: fields[0].to_vec(),
                    object_id: fields[1].to_vec(),
                },
            )
            .is_some()
        {
            return Err("Git returned duplicate conflict stages".to_owned());
        }
    }
    Ok(stages)
}

fn has_conflict_stages(stages: &BTreeMap<u8, ConflictStage>) -> bool {
    stages.keys().any(|stage| *stage > 0)
}

fn resolve_conflict_side(
    path: &Path,
    path_bytes: &[u8],
    side: ConflictSide,
) -> Result<RepositoryStatus, String> {
    if path_bytes.is_empty() || path_bytes.len() > MAX_GIT_STDOUT_BYTES || path_bytes.contains(&0) {
        return Err("Invalid Git path".to_owned());
    }
    let status = repository_status(path)?;
    if !status
        .unstaged
        .iter()
        .any(|entry| entry.kind == StatusKind::Conflicting && entry.path_bytes == path_bytes)
    {
        return Err("The selected file is no longer in conflict".to_owned());
    }

    let stages = conflict_stages(path, path_bytes)?;
    let stage = match side {
        ConflictSide::Current => 2,
        ConflictSide::Incoming => 3,
    };
    let side_exists = stages.contains_key(&stage);
    // A both-deleted (DD) conflict only has the base stage; resolving it removes the path.
    if !has_conflict_stages(&stages) {
        return Err("The selected file is no longer in conflict".to_owned());
    }
    let expected = stages.get(&stage).map(|entry| ConflictStage {
        mode: entry.mode.clone(),
        object_id: entry.object_id.clone(),
    });

    let path_argument = git_path_os_string(path_bytes)?;
    // shortcut: Git CLI has no conditional index transaction; compare the full snapshot before mutation.
    if conflict_stages(path, path_bytes)? != stages {
        return Err("Conflict changed while resolving; review the file again".to_owned());
    }
    let status = repository_status(path)?;
    if !status
        .unstaged
        .iter()
        .any(|entry| entry.kind == StatusKind::Conflicting && entry.path_bytes == path_bytes)
    {
        return Err("The selected file is no longer in conflict".to_owned());
    }
    let args = if side_exists {
        match side {
            ConflictSide::Current => vec!["--literal-pathspecs", "checkout", "--ours", "--"],
            ConflictSide::Incoming => vec!["--literal-pathspecs", "checkout", "--theirs", "--"],
        }
    } else {
        vec!["--literal-pathspecs", "rm", "--force", "--"]
    };
    let output = git_output_with_paths(path, &args, &[path_argument.as_os_str()], false)?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    if side_exists {
        let expected = expected
            .as_ref()
            .ok_or_else(|| "The selected conflict side is unavailable".to_owned())?;
        let current_stages = conflict_stages(path, path_bytes)?;
        if !current_stages.get(&stage).is_some_and(|entry| {
            entry.mode == expected.mode && entry.object_id == expected.object_id
        }) {
            return Err("Conflict changed while resolving; review the file again".to_owned());
        }
        let args = ["--literal-pathspecs", "add", "--"];
        let output = git_output_with_paths(path, &args, &[path_argument.as_os_str()], false)?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
        }
        let current_stages = conflict_stages(path, path_bytes)?;
        if current_stages.len() != 1
            || !current_stages.get(&0).is_some_and(|entry| {
                entry.mode == expected.mode && entry.object_id == expected.object_id
            })
        {
            return Err("File changed while resolving; review its staged content".to_owned());
        }
    } else if !conflict_stages(path, path_bytes)?.is_empty() {
        return Err("Index changed while resolving; review the file again".to_owned());
    }
    let status = repository_status(path)?;
    if status
        .unstaged
        .iter()
        .any(|entry| entry.path_bytes == path_bytes)
    {
        return Err("Worktree changed while resolving; review the file again".to_owned());
    }
    Ok(status)
}

fn conflict_versions(
    path: &Path,
    path_bytes: &[u8],
) -> Result<(ConflictVersion, ConflictVersion), String> {
    let stages = conflict_stages(path, path_bytes)?;
    if !has_conflict_stages(&stages) {
        return Err("The selected file is no longer in conflict".to_owned());
    }

    let mut versions = Vec::with_capacity(2);
    for stage in [stages.get(&2), stages.get(&3)] {
        let Some(stage) = stage else {
            versions.push(ConflictVersion {
                kind: ConflictVersionKind::Deleted,
                content: None,
            });
            continue;
        };
        let object_id = String::from_utf8(stage.object_id.clone())
            .map_err(|_| "Git returned an invalid conflict object ID".to_owned())?;
        if stage.mode == b"160000" {
            versions.push(ConflictVersion {
                kind: ConflictVersionKind::Submodule,
                content: Some(object_id),
            });
            continue;
        }
        let (output, truncated) =
            git_output_with_paths_capped(path, &["cat-file", "blob", &object_id], &[])?;
        if truncated {
            versions.push(ConflictVersion {
                kind: ConflictVersionKind::TooLarge,
                content: None,
            });
            continue;
        }
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
        }
        if output.stdout.contains(&0) {
            versions.push(ConflictVersion {
                kind: ConflictVersionKind::Binary,
                content: None,
            });
        } else {
            versions.push(ConflictVersion {
                kind: ConflictVersionKind::Text,
                content: Some(String::from_utf8_lossy(&output.stdout).into_owned()),
            });
        }
    }
    let [current, incoming] = versions
        .try_into()
        .map_err(|_| "Git returned an invalid number of conflict sides".to_owned())?;
    Ok((current, incoming))
}

fn file_diff(path: &Path, path_bytes: &[u8], group: StatusGroup) -> Result<FileDiff, String> {
    if path_bytes.is_empty() || path_bytes.len() > MAX_GIT_STDOUT_BYTES || path_bytes.contains(&0) {
        return Err("Invalid Git path".to_owned());
    }
    let status = repository_status(path)?;
    let entry = match group {
        StatusGroup::Staged => status
            .staged
            .iter()
            .find(|entry| entry.path_bytes == path_bytes),
        StatusGroup::Unstaged => status
            .unstaged
            .iter()
            .find(|entry| entry.path_bytes == path_bytes),
    }
    .ok_or_else(|| "The file is no longer in this change list".to_owned())?;
    if entry.kind == StatusKind::Conflicting {
        let (current, incoming) = conflict_versions(path, path_bytes)?;
        return Ok(FileDiff {
            kind: FileDiffKind::Conflict,
            patch: None,
            current: Some(current),
            incoming: Some(incoming),
        });
    }
    let untracked = group == StatusGroup::Unstaged && entry.kind == StatusKind::Added;
    let path_argument = git_path_os_string(path_bytes)?;
    let mut args = vec!["--literal-pathspecs", "diff"];
    if untracked {
        args.push("--no-index");
    } else if group == StatusGroup::Staged {
        args.push("--cached");
    }
    args.extend([
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--no-relative",
        "--no-renames",
        "--submodule=short",
        "--unified=3",
    ]);
    args.push("--");
    #[cfg(windows)]
    let null_device = OsStr::new("NUL");
    #[cfg(not(windows))]
    let null_device = OsStr::new("/dev/null");
    let paths = if untracked {
        vec![null_device, path_argument.as_os_str()]
    } else {
        vec![path_argument.as_os_str()]
    };
    let output = git_output_with_paths(path, &args, &paths, true)?;
    if !(output.status.success() || untracked && output.status.code() == Some(1)) {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let is_binary = output.stdout.split(|byte| *byte == b'\n').any(|line| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        line.starts_with(b"Binary files ") && line.ends_with(b" differ")
    });
    if is_binary {
        return Ok(FileDiff {
            kind: FileDiffKind::Binary,
            patch: None,
            current: None,
            incoming: None,
        });
    }
    let Ok(patch) = String::from_utf8(output.stdout) else {
        return Ok(FileDiff {
            kind: FileDiffKind::Binary,
            patch: None,
            current: None,
            incoming: None,
        });
    };
    let kind = if patch.lines().any(|line| line.starts_with("Submodule ")) {
        FileDiffKind::Submodule
    } else {
        FileDiffKind::Text
    };
    Ok(FileDiff {
        kind,
        patch: Some(patch),
        current: None,
        incoming: None,
    })
}

fn stage_status_path(path: &Path, path_bytes: &[u8]) -> Result<RepositoryStatus, String> {
    if path_bytes.is_empty() || path_bytes.len() > MAX_GIT_STDOUT_BYTES || path_bytes.contains(&0) {
        return Err("Invalid Git path".to_owned());
    }
    let status = repository_status(path)?;
    let Some(entry) = status
        .unstaged
        .iter()
        .find(|entry| entry.path_bytes == path_bytes)
    else {
        return Err("The file is no longer unstaged".to_owned());
    };
    let relative_path = git_path_os_string(path_bytes)?;
    let filesystem_path: PathBuf = path.join(relative_path);
    // shortcut: Git CLI cannot atomically stage from a checked worktree path; use index plumbing if concurrent writers must be supported.
    match filesystem_path.symlink_metadata() {
        Ok(metadata) if metadata.file_type().is_dir() => {
            if !is_gitlink(path, path_bytes)? {
                return Err("The selected file is now a directory".to_owned());
            }
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Could not inspect selected file: {error}")),
    }
    let mut pathspec = path_bytes.to_vec();
    pathspec.push(0);
    let mut args = vec!["--literal-pathspecs", "add"];
    if entry.kind == StatusKind::Removed {
        args.push("--update");
    }
    args.extend(["--pathspec-from-file=-", "--pathspec-file-nul"]);
    let output = git_output_with_input(path, &args, &pathspec)?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    repository_status(path)
}

fn unstage_status_path(path: &Path, path_bytes: &[u8]) -> Result<RepositoryStatus, String> {
    if path_bytes.is_empty() || path_bytes.len() > MAX_GIT_STDOUT_BYTES || path_bytes.contains(&0) {
        return Err("Invalid Git path".to_owned());
    }
    let status = repository_status(path)?;
    let Some(entry) = status
        .staged
        .iter()
        .find(|entry| entry.path_bytes == path_bytes)
    else {
        return Err("The file is no longer staged".to_owned());
    };

    let mut pathspec = path_bytes.to_vec();
    pathspec.push(0);
    let mut args = vec!["--literal-pathspecs"];
    if entry.kind == StatusKind::Added {
        args.extend(["rm", "--cached", "-f"]);
    } else {
        args.push("reset");
    }
    args.extend(["--pathspec-from-file=-", "--pathspec-file-nul"]);
    let output = git_output_with_input(path, &args, &pathspec)?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    repository_status(path)
}

fn graph_reservations_for(state: &mut GraphState, hash: &str) -> Vec<usize> {
    let lanes = state
        .reserved_lanes
        .iter()
        .filter_map(|(lane, reserved)| (reserved == hash).then_some(*lane))
        .collect::<Vec<_>>();
    for lane in &lanes {
        state.reserved_lanes.remove(lane);
    }
    lanes
}

fn next_available_lane(state: &GraphState, min: usize) -> usize {
    (min..)
        .find(|lane| !state.reserved_lanes.contains_key(lane))
        .unwrap()
}

fn validate_graph_state(state: &GraphState) -> Result<(), String> {
    if state.reserved_lanes.len() > MAX_GRAPH_LANES
        || state
            .reserved_lanes
            .iter()
            .any(|(lane, hash)| *lane >= MAX_GRAPH_LANES || !is_commit_id(hash))
    {
        return Err("Invalid commit graph cursor".to_owned());
    }
    Ok(())
}

fn layout_commits(commits: &mut [Commit], state: &mut GraphState) {
    for commit in commits {
        let reserved_lanes = graph_reservations_for(state, &commit.id);
        let child_count = reserved_lanes.len();
        let lane = reserved_lanes
            .first()
            .copied()
            .unwrap_or_else(|| next_available_lane(state, 0));
        let passing_lanes = state.reserved_lanes.keys().copied().collect::<Vec<_>>();
        let mut merging_lanes = Vec::new();

        if let Some(first_parent) = commit.parents.first() {
            state.reserved_lanes.insert(lane, first_parent.clone());
            for parent in commit.parents.iter().skip(1) {
                let existing = state
                    .reserved_lanes
                    .iter()
                    .find_map(|(lane, reserved)| (reserved == parent).then_some(*lane));
                let merge_lane = existing.unwrap_or_else(|| {
                    let merge_lane = next_available_lane(state, lane);
                    state.reserved_lanes.insert(merge_lane, parent.clone());
                    merge_lane
                });
                merging_lanes.push(merge_lane);
            }
        }

        commit.lane = lane;
        commit.passing_lanes = passing_lanes;
        commit.merging_lanes = merging_lanes;
        commit.forking_off_lanes = reserved_lanes.into_iter().skip(1).collect();
        commit.child_count = child_count;
    }
}

fn current_branch(path: &Path) -> Result<String, String> {
    let output = git_output(path, &["symbolic-ref", "--quiet", "--short", "HEAD"], None)?;

    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end_matches(['\r', '\n'])
            .to_owned());
    }
    if output.stderr.is_empty() {
        return Ok("Detached HEAD".to_owned());
    }
    Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
}

fn repository_root(path: &Path) -> Result<String, String> {
    if !path.is_dir() {
        return Err("Selected path is not a directory".to_owned());
    }
    let output = git_output(path, &["rev-parse", "--show-toplevel"], Some("C"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end_matches(['\r', '\n'])
            .to_owned());
    }
    let error = String::from_utf8_lossy(&output.stderr);
    if error.contains("not a git repository") {
        return Err(
            "Failed to open repository, could not find a repository in the specified path"
                .to_owned(),
        );
    }
    Err(error.trim().to_owned())
}

fn git_path(path: &Path, name: &str) -> Result<PathBuf, String> {
    let output = git_text(
        path,
        &["rev-parse", "--path-format=absolute", "--git-path", name],
    )?;
    Ok(PathBuf::from(output))
}

fn config_value(path: &Path, scope: &str, key: &str) -> Option<String> {
    // --includes: a scope flag turns include.path off by default, but identities are often kept in included files.
    let output = git_output(path, &["config", scope, "--includes", "--get", key], None).ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_owned()
    })
}

fn read_identity(path: &Path, scope: &str) -> CommitIdentity {
    CommitIdentity {
        name: config_value(path, scope, "user.name"),
        email: config_value(path, scope, "user.email"),
    }
}

fn commit_message_metadata(path: &Path) -> Result<CommitMetadata, String> {
    let merging = merge_in_progress(path)?;
    let message_path = if merging {
        git_path(path, "MERGE_MSG")?
    } else {
        git_path(path, "COMMIT_EDITMSG")?
    };
    // Lossy: a message written by another tool in a legacy encoding must not block opening the repository.
    let draft = match fs::read(message_path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("Could not read commit message: {error}")),
    };
    let without_comments = |message: &str| {
        message
            .lines()
            .filter(|line| !line.starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_owned()
    };
    let previous_message = history_head(path)?
        .map(|_| git_text(path, &["log", "-1", "--format=%B"]))
        .transpose()?;
    let draft = if merging {
        without_comments(&draft)
    } else if previous_message.as_deref().is_some_and(|previous| {
        // Git rewrites COMMIT_EDITMSG on every commit, including the ones made in a terminal;
        // a draft equal to the last commit message is that leftover, not unsent work.
        draft.trim() == previous.trim() || without_comments(&draft) == previous.trim()
    }) {
        String::new()
    } else {
        draft
    };
    Ok(CommitMetadata {
        draft,
        previous_message,
        local_identity: read_identity(path, "--local"),
        global_identity: read_identity(path, "--global"),
    })
}

fn write_commit_message(path: &Path, message: &str) -> Result<(), String> {
    let message_path = if merge_in_progress(path)? {
        git_path(path, "MERGE_MSG")?
    } else {
        git_path(path, "COMMIT_EDITMSG")?
    };
    if message.is_empty() {
        match fs::remove_file(message_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("Could not clear commit message: {error}")),
        }
    } else {
        fs::write(message_path, message)
            .map_err(|error| format!("Could not save commit message: {error}"))
    }
}

fn commit_sign_off(
    path: &Path,
    message: &str,
    identity: &CommitIdentity,
) -> Result<String, String> {
    let config_path = git_path(path, "gitnuro")?;
    if !config_path.exists() {
        return Ok(message.to_owned());
    }
    let config_argument = config_path.to_string_lossy();
    let enabled = git_output(
        path,
        &[
            "config",
            "--file",
            &config_argument,
            "--bool",
            "--get",
            "signoff.enabled",
        ],
        None,
    )?;
    if !enabled.status.success() {
        let error = String::from_utf8_lossy(&enabled.stderr).trim().to_owned();
        return if error.is_empty() {
            Ok(message.to_owned())
        } else {
            Err(error)
        };
    }
    if !String::from_utf8_lossy(&enabled.stdout)
        .trim()
        .eq_ignore_ascii_case("true")
    {
        return Ok(message.to_owned());
    }
    let format = git_output(
        path,
        &[
            "config",
            "--file",
            &config_argument,
            "--get",
            "signoff.format",
        ],
        None,
    )?;
    let format = if format.status.success() {
        String::from_utf8_lossy(&format.stdout)
            .trim_end()
            .to_owned()
    } else {
        let error = String::from_utf8_lossy(&format.stderr).trim().to_owned();
        if !error.is_empty() {
            return Err(error);
        }
        "Signed-off-by: %user <%email>".to_owned()
    };
    let signature = format
        .replace("%user", identity.name.as_deref().unwrap_or_default())
        .replace("%email", identity.email.as_deref().unwrap_or_default());
    // An amend starts from the previous message, which already carries the sign-off.
    if message.lines().any(|line| line.trim() == signature.trim()) {
        return Ok(message.to_owned());
    }
    Ok(format!("{message}\n\n{signature}"))
}

fn set_global_identity(path: &Path, identity: &CommitIdentity) -> Result<(), String> {
    for (key, value) in [
        ("user.name", identity.name.as_deref().unwrap_or_default()),
        ("user.email", identity.email.as_deref().unwrap_or_default()),
    ] {
        let args = [
            "config".to_owned(),
            "--global".to_owned(),
            "--replace-all".to_owned(),
            key.to_owned(),
            value.to_owned(),
        ];
        let output = git_output(
            path,
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
            None,
        )?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
        }
    }
    Ok(())
}

fn commit_changes(
    path: &Path,
    message: &str,
    amend: bool,
    identity: Option<CommitIdentity>,
    persist_identity: bool,
) -> Result<RepositorySummary, String> {
    if message.is_empty() {
        return Err("Commit message cannot be empty".to_owned());
    }
    let status = repository_status(path)?;
    // A merge whose result equals HEAD leaves nothing staged but still has to be concluded.
    if !amend && status.staged.is_empty() && !status.merging {
        return Err("Stage changes before committing".to_owned());
    }
    if amend && history_head(path)?.is_none() {
        return Err("There is no commit to amend".to_owned());
    }

    let metadata = commit_message_metadata(path)?;
    let override_identity = identity.is_some();
    let resolved_identity = identity.unwrap_or_else(|| CommitIdentity {
        name: metadata
            .local_identity
            .name
            .clone()
            .or(metadata.global_identity.name.clone()),
        email: metadata
            .local_identity
            .email
            .clone()
            .or(metadata.global_identity.email.clone()),
    });
    if resolved_identity
        .name
        .as_deref()
        .unwrap_or_default()
        .is_empty()
        || resolved_identity
            .email
            .as_deref()
            .unwrap_or_default()
            .is_empty()
    {
        return Err("Git author name and email are required".to_owned());
    }
    if persist_identity {
        set_global_identity(path, &resolved_identity)?;
    }

    let message = commit_sign_off(path, message, &resolved_identity)?;
    let merging = status.merging;
    let mut command = git_command();
    command
        .arg("-C")
        .arg(path)
        .arg("-c")
        .arg(format!(
            "user.name={}",
            resolved_identity.name.as_deref().unwrap()
        ))
        .arg("-c")
        .arg(format!(
            "user.email={}",
            resolved_identity.email.as_deref().unwrap()
        ))
        .arg("commit");
    if amend {
        command.args(["--amend", "--allow-empty"]);
        if override_identity {
            command.arg(format!(
                "--author={} <{}>",
                resolved_identity.name.as_deref().unwrap(),
                resolved_identity.email.as_deref().unwrap()
            ));
        }
    } else if merging {
        command.arg("--allow-empty");
    }
    command
        .args(["--file", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start Git commit: {error}"))?;
    let (output, _) = collect_output_inner(
        &mut child,
        Some(message.as_bytes()),
        false,
        false,
        MAX_GIT_STDOUT_BYTES,
        GIT_COMMIT_TIMEOUT,
    )?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let error = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if stdout.is_empty() && error.is_empty() {
            format!("Git commit failed with {}", output.status)
        } else {
            [stdout, error]
                .into_iter()
                .filter(|message| !message.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        });
    }
    write_commit_message(path, "")?;

    repository_summary(path)
}

fn history_head(path: &Path) -> Result<Option<String>, String> {
    let output = git_output(
        path,
        &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
        None,
    )?;
    if output.status.success() {
        return Ok(Some(
            String::from_utf8_lossy(&output.stdout)
                .trim_end_matches(['\r', '\n'])
                .to_owned(),
        ));
    }
    if output.stderr.is_empty() {
        return Ok(None);
    }
    Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
}

fn history_refs(path: &Path, head: Option<&str>) -> Result<HistoryRefs, String> {
    let refs = git_text_with_stdout_limit(
        path,
        &[
            "for-each-ref",
            "--format=%(*objectname)%00%(*objecttype)%00%(objectname)%00%(objecttype)%00%(refname)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
        MAX_HISTORY_REFS_BYTES,
    )?;
    let mut tips = BTreeSet::new();
    let mut references = Vec::new();
    if let Some(head) = head {
        tips.insert(head.to_owned());
    }
    for record in refs.lines() {
        let fields = record.split('\0').collect::<Vec<_>>();
        if fields.len() != 5 {
            continue;
        }
        let tip = if fields[1] == "commit" {
            fields[0]
        } else if fields[3] == "commit" {
            fields[2]
        } else {
            continue;
        };
        if is_commit_id(tip) {
            tips.insert(tip.to_owned());
            let refname = fields[4];
            let (kind, name) = if let Some(name) = refname.strip_prefix("refs/heads/") {
                (ReferenceKind::Local, name)
            } else if let Some(name) = refname.strip_prefix("refs/remotes/") {
                (ReferenceKind::Remote, name)
            } else if let Some(name) = refname.strip_prefix("refs/tags/") {
                (ReferenceKind::Tag, name)
            } else {
                continue;
            };
            references.push(GitReference {
                id: tip.to_owned(),
                name: name.to_owned(),
                kind,
            });
        }
    }
    references.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(HistoryRefs {
        tips: tips.into_iter().collect(),
        references,
    })
}

fn is_commit_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn commit_page(
    path: &Path,
    history_refs: &HistoryRefs,
    skip: usize,
    graph_state: GraphState,
) -> Result<CommitPage, String> {
    if history_refs.tips.is_empty() {
        return Ok(page_commits(Vec::new(), graph_state));
    }
    if history_refs.tips.iter().any(|tip| !is_commit_id(tip))
        || history_refs.references.iter().any(|reference| {
            !is_commit_id(&reference.id)
                || reference.name.is_empty()
                || reference.name.len() > 1024
                || reference.name.chars().any(char::is_control)
                || !history_refs.tips.contains(&reference.id)
        })
    {
        return Err("Invalid history commit ID".to_owned());
    }

    let skip_arg = format!("--skip={skip}");
    let limit_arg = format!("--max-count={}", COMMIT_PAGE_SIZE + 1);
    let revisions = history_refs.tips.join("\n");
    if revisions.len() > MAX_HISTORY_REFS_BYTES {
        return Err("Too many history references".to_owned());
    }
    let mut revisions = revisions.into_bytes();
    revisions.push(b'\n');
    let args = [
        "log",
        "--stdin",
        "--date-order",
        skip_arg.as_str(),
        limit_arg.as_str(),
        "--format=%H%x00%P%x00%<(256,trunc)%s%x00%<(256,trunc)%an%x00%aI%x00",
    ];
    let output = git_text_with_input(path, &args, &revisions)?;
    let mut commits = parse_commits(&output);
    for commit in &mut commits {
        commit.references = history_refs
            .references
            .iter()
            .filter(|reference| reference.id == commit.id)
            .cloned()
            .collect();
    }
    Ok(page_commits(commits, graph_state))
}

fn repository_summary(path: &Path) -> Result<RepositorySummary, String> {
    let root_path = repository_root(path)?;
    let root = Path::new(&root_path);
    let history_head = history_head(root)?;
    let history_refs = history_refs(root, history_head.as_deref())?;
    let page = commit_page(root, &history_refs, 0, GraphState::default())?;
    Ok(RepositorySummary {
        path: root.display().to_string(),
        name: root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Repository")
            .to_owned(),
        branch: current_branch(root)?,
        history_head,
        history_refs,
        status: repository_status(root)?,
        commit: commit_message_metadata(root)?,
        commits: page.commits,
        has_more_commits: page.has_more_commits,
        graph_state: page.graph_state,
    })
}

fn page_commits(mut commits: Vec<Commit>, mut graph_state: GraphState) -> CommitPage {
    let has_more_commits = commits.len() > COMMIT_PAGE_SIZE;
    commits.truncate(COMMIT_PAGE_SIZE);
    layout_commits(&mut commits, &mut graph_state);
    CommitPage {
        commits,
        has_more_commits,
        graph_state,
    }
}

/// What the backend remembers between IPC calls. Every command except `open_repository` only acts on the
/// repository opened through it, so a compromised page cannot point mutations at an arbitrary directory.
#[derive(Clone, Default)]
struct AppState(Arc<AppStateInner>);

#[derive(Default)]
struct AppStateInner {
    open_root: Mutex<Option<String>>,
    /// Serializes index and COMMIT_EDITMSG writers (stage, unstage, resolve, commit, draft saves).
    mutation: Mutex<()>,
}

impl AppState {
    fn set_open_root(&self, root: String) {
        *self
            .0
            .open_root
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(root);
    }

    /// Resolves `path` to its repository root and requires it to be the open repository.
    fn open_root(&self, path: &str) -> Result<String, String> {
        let root = repository_root(Path::new(path))?;
        let open = self
            .0
            .open_root
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if open.as_deref() == Some(root.as_str()) {
            Ok(root)
        } else {
            Err("This repository is not the open repository; open it again".to_owned())
        }
    }

    fn lock_mutations(&self) -> MutexGuard<'_, ()> {
        self.0
            .mutation
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

#[tauri::command]
async fn open_repository(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<RepositorySummary, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let summary = repository_summary(Path::new(&path))?;
        state.set_open_root(repository_root(Path::new(&summary.path))?);
        Ok(summary)
    })
    .await
    .map_err(|error| format!("Could not read repository: {error}"))?
}

#[tauri::command]
async fn persist_commit_message(
    state: tauri::State<'_, AppState>,
    path: String,
    message: String,
) -> Result<(), String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _mutation = state.lock_mutations();
        let root = state.open_root(&path)?;
        write_commit_message(Path::new(&root), &message)
    })
    .await
    .map_err(|error| format!("Could not save commit message: {error}"))?
}

#[tauri::command]
async fn commit_repository(
    state: tauri::State<'_, AppState>,
    path: String,
    message: String,
    amend: bool,
    identity: Option<CommitIdentity>,
    persist_identity: bool,
) -> Result<RepositorySummary, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _mutation = state.lock_mutations();
        let root = state.open_root(&path)?;
        commit_changes(
            Path::new(&root),
            &message,
            amend,
            identity,
            persist_identity,
        )
    })
    .await
    .map_err(|error| format!("Could not create commit: {error}"))?
}

#[tauri::command]
async fn refresh_repository_status(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<RepositoryStatus, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let root = state.open_root(&path)?;
        repository_status(Path::new(&root))
    })
    .await
    .map_err(|error| format!("Could not refresh repository status: {error}"))?
}

#[tauri::command]
async fn get_file_diff(
    state: tauri::State<'_, AppState>,
    path: String,
    path_bytes: Vec<u8>,
    group: StatusGroup,
) -> Result<FileDiff, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let root = state.open_root(&path)?;
        file_diff(Path::new(&root), &path_bytes, group)
    })
    .await
    .map_err(|error| format!("Could not load file diff: {error}"))?
}

#[tauri::command]
async fn resolve_conflict_file(
    state: tauri::State<'_, AppState>,
    path: String,
    path_bytes: Vec<u8>,
    side: ConflictSide,
) -> Result<RepositoryStatus, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _mutation = state.lock_mutations();
        let root = state.open_root(&path)?;
        resolve_conflict_side(Path::new(&root), &path_bytes, side)
    })
    .await
    .map_err(|error| format!("Could not resolve conflict: {error}"))?
}

#[tauri::command]
async fn stage_status_entry(
    state: tauri::State<'_, AppState>,
    path: String,
    path_bytes: Vec<u8>,
) -> Result<RepositoryStatus, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _mutation = state.lock_mutations();
        let root = state.open_root(&path)?;
        stage_status_path(Path::new(&root), &path_bytes)
    })
    .await
    .map_err(|error| format!("Could not stage file: {error}"))?
}

#[tauri::command]
async fn unstage_status_entry(
    state: tauri::State<'_, AppState>,
    path: String,
    path_bytes: Vec<u8>,
) -> Result<RepositoryStatus, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _mutation = state.lock_mutations();
        let root = state.open_root(&path)?;
        unstage_status_path(Path::new(&root), &path_bytes)
    })
    .await
    .map_err(|error| format!("Could not unstage file: {error}"))?
}

#[tauri::command]
async fn load_more_commits(
    state: tauri::State<'_, AppState>,
    path: String,
    history_refs: HistoryRefs,
    skip: usize,
    graph_state: GraphState,
) -> Result<CommitPage, String> {
    validate_graph_state(&graph_state)?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let root = state.open_root(&path)?;
        commit_page(Path::new(&root), &history_refs, skip, graph_state)
    })
    .await
    .map_err(|error| format!("Could not read repository: {error}"))?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            open_repository,
            load_more_commits,
            refresh_repository_status,
            persist_commit_message,
            commit_repository,
            get_file_diff,
            resolve_conflict_file,
            stage_status_entry,
            unstage_status_entry
        ])
        .run(tauri::generate_context!())
        .expect("failed to run GitMiro");
}

#[cfg(test)]
mod tests {
    use super::{
        COMMIT_PAGE_SIZE, Commit, CommitIdentity, ConflictSide, ConflictVersionKind, FileDiffKind,
        GraphState, MAX_GIT_STDERR_BYTES, MAX_GIT_STDOUT_BYTES, StatusGroup, StatusKind,
        collect_output, commit_changes, commit_message_metadata, commit_page, current_branch,
        file_diff, history_head, history_refs, is_commit_id, is_gitlink, page_commits,
        parse_commits, parse_status, read_capped, repository_root, repository_status,
        resolve_conflict_side, stage_status_path, unstage_status_path, validate_graph_state,
        write_commit_message,
    };

    #[test]
    fn caps_reader_output_and_reports_truncation() {
        let (bytes, truncated) = read_capped(&b"abcdef"[..], 4).unwrap();

        assert_eq!(bytes, b"abcd");
        assert!(truncated);
    }

    #[test]
    fn parses_porcelain_v2_staged_unstaged_untracked_conflict_and_rename_paths() {
        let output = b"1 M. N... 100644 100644 100644 abc abc staged file.txt\0\
1 .M N... 100644 100644 100644 abc abc worktree.txt\0\
? new\nfile.txt\0\
u UU N... 100644 100644 100644 100644 abc abc abc conflict.txt\0\
2 R. N... 100644 100644 100644 abc abc R100 renamed file.txt\0old file.txt\0";

        let status = parse_status(output, false).unwrap();

        assert_eq!(status.staged.len(), 3);
        assert_eq!(status.staged[0].path, "old file.txt");
        assert!(matches!(status.staged[0].kind, StatusKind::Removed));
        assert_eq!(status.staged[1].path, "renamed file.txt");
        assert!(matches!(status.staged[1].kind, StatusKind::Added));
        assert_eq!(status.staged[2].path, "staged file.txt");
        assert!(matches!(status.staged[2].kind, StatusKind::Modified));
        assert_eq!(status.unstaged.len(), 3);
        assert_eq!(status.unstaged[0].path, "conflict.txt");
        assert!(matches!(status.unstaged[0].kind, StatusKind::Conflicting));
        assert_eq!(status.unstaged[1].path, "new\\nfile.txt");
        assert_eq!(status.unstaged[1].path_bytes, b"new\nfile.txt");
        assert!(matches!(status.unstaged[1].kind, StatusKind::Added));
        assert_eq!(status.unstaged[2].path, "worktree.txt");
        assert!(matches!(status.unstaged[2].kind, StatusKind::Modified));
    }

    #[test]
    fn rejects_malformed_porcelain_v2_records() {
        assert!(parse_status(b"1 M. missing fields\0", false).is_err());
        assert!(parse_status(b"2 R. missing original path\0", false).is_err());
    }

    #[test]
    fn preserves_non_utf8_paths_and_marks_capped_status_output() {
        let status = parse_status(b"? file-\xff.txt\0? file-\\xFF.txt\0? partial", true).unwrap();

        assert!(status.truncated);
        assert_eq!(status.unstaged.len(), 2);
        assert_eq!(status.unstaged[0].path, "file-\\\\xFF.txt");
        assert_eq!(status.unstaged[1].path, "file-\\xFF.txt");
        assert_eq!(status.unstaged[1].path_bytes, b"file-\xff.txt");
    }

    #[test]
    fn drops_incomplete_rename_from_capped_status_output() {
        let status = parse_status(
            b"? complete.txt\x002 R. N... 100644 100644 100644 abc abc R100 target.txt\0",
            true,
        )
        .unwrap();

        assert_eq!(status.unstaged.len(), 1);
        assert_eq!(status.unstaged[0].path, "complete.txt");
    }

    #[test]
    fn commits_only_staged_changes_amends_previous_commit_and_persists_draft() {
        use std::{
            fs,
            process::Command,
            time::{SystemTime, UNIX_EPOCH},
        };

        let path = std::env::temp_dir().join(format!(
            "gitmiro-commit-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        let run_git = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            output
        };
        run_git(&["init", "--quiet"]);
        run_git(&["config", "user.name", "GitMiro Test"]);
        run_git(&["config", "user.email", "gitmiro-test@example.invalid"]);
        run_git(&["config", "commit.gpgsign", "false"]);
        fs::write(path.join("tracked.txt"), "base\n").unwrap();
        run_git(&["add", "tracked.txt"]);
        run_git(&["commit", "--quiet", "-m", "initial"]);

        fs::write(path.join("tracked.txt"), "staged\n").unwrap();
        run_git(&["add", "tracked.txt"]);
        fs::write(path.join("tracked.txt"), "unstaged\n").unwrap();
        write_commit_message(&path, "draft message").unwrap();
        assert!(matches!(
            commit_changes(&path, "", false, None, false),
            Err(error) if error == "Commit message cannot be empty"
        ));
        assert_eq!(
            commit_message_metadata(&path).unwrap().draft,
            "draft message"
        );

        fs::write(
            path.join(".git/gitnuro"),
            "[signoff]\n  enabled = true\n  format = Reviewed-by: %user <%email>\n",
        )
        .unwrap();
        commit_changes(&path, "feature", false, None, false).unwrap();
        let message = String::from_utf8(run_git(&["log", "-1", "--format=%B"]).stdout).unwrap();
        assert!(
            message.contains("feature\n\nReviewed-by: GitMiro Test <gitmiro-test@example.invalid>")
        );
        assert_eq!(commit_message_metadata(&path).unwrap().draft, "");
        assert_eq!(repository_status(&path).unwrap().staged.len(), 0);
        assert_eq!(repository_status(&path).unwrap().unstaged.len(), 1);

        commit_changes(
            &path,
            "amended",
            true,
            Some(CommitIdentity {
                name: Some("Override".into()),
                email: Some("override@example.invalid".into()),
            }),
            false,
        )
        .unwrap();
        let amended = run_git(&["log", "-1", "--format=%an <%ae>%n%s"]);
        assert_eq!(
            String::from_utf8(amended.stdout).unwrap().trim(),
            "Override <override@example.invalid>\namended"
        );
        assert_eq!(repository_status(&path).unwrap().unstaged.len(), 1);
        let _ = fs::remove_dir_all(&path);
    }

    #[test]
    fn returns_hook_stderr_and_keeps_commit_draft_on_rejection() {
        use std::{
            fs,
            process::Command,
            time::{SystemTime, UNIX_EPOCH},
        };

        let path = std::env::temp_dir().join(format!(
            "gitmiro-hook-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        let run_git = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            output
        };
        run_git(&["init", "--quiet"]);
        run_git(&["config", "user.name", "GitMiro Test"]);
        run_git(&["config", "user.email", "gitmiro-test@example.invalid"]);
        run_git(&["config", "commit.gpgsign", "false"]);
        fs::create_dir(path.join("hooks")).unwrap();
        let hook = path.join("hooks/pre-commit");
        fs::write(
            &hook,
            "#!/bin/sh\necho 'hook stdout reason'\necho 'hook stderr reason' >&2\nexit 1\n",
        )
        .unwrap();
        #[cfg(unix)]
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        run_git(&["config", "core.hooksPath", "hooks"]);
        fs::write(path.join("staged.txt"), "staged\n").unwrap();
        run_git(&["add", "staged.txt"]);
        write_commit_message(&path, "draft survives").unwrap();

        let error = match commit_changes(&path, "draft survives", false, None, false) {
            Ok(_) => panic!("hook should reject the commit"),
            Err(error) => error,
        };

        assert!(error.contains("hook stdout reason"));
        assert!(error.contains("hook stderr reason"));
        assert_eq!(
            commit_message_metadata(&path).unwrap().draft,
            "draft survives"
        );
        assert_eq!(repository_status(&path).unwrap().staged.len(), 1);
        let _ = fs::remove_dir_all(&path);
    }

    #[test]
    fn stages_only_the_requested_literal_status_path() {
        use std::{
            fs,
            process::Command,
            time::{SystemTime, UNIX_EPOCH},
        };

        let path = std::env::temp_dir().join(format!(
            "gitmiro-status-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        let run_git = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        run_git(&["init", "--quiet"]);
        fs::create_dir(path.join(".git/gitmiro-test-hooks")).unwrap();
        let excludes_path = path.join(".git/gitmiro-test-excludes");
        fs::write(&excludes_path, "").unwrap();
        run_git(&["config", "core.hooksPath", ".git/gitmiro-test-hooks"]);
        run_git(&[
            "config",
            "core.excludesFile",
            excludes_path.to_str().unwrap(),
        ]);
        run_git(&["config", "commit.gpgsign", "false"]);
        run_git(&["config", "user.name", "GitMiro Test"]);
        run_git(&["config", "user.email", "gitmiro-test@example.invalid"]);
        fs::write(path.join("tracked.txt"), "original\n").unwrap();
        run_git(&["add", "tracked.txt"]);
        run_git(&["commit", "--quiet", "-m", "initial"]);
        fs::write(path.join("tracked.txt"), "staged\n").unwrap();
        run_git(&["add", "tracked.txt"]);
        fs::write(path.join("tracked.txt"), "unstaged\n").unwrap();
        fs::write(
            path.join("new.txt"),
            "Binary files are ordinary words here\nSubmodule description is text too\n",
        )
        .unwrap();
        fs::write(path.join("glob[1].txt"), "literal\n").unwrap();
        fs::write(path.join("glob1.txt"), "match\n").unwrap();

        let status = repository_status(&path).unwrap();

        assert_eq!(status.staged.len(), 1);
        assert_eq!(status.staged[0].path, "tracked.txt");
        assert!(matches!(status.staged[0].kind, StatusKind::Modified));
        assert_eq!(status.unstaged.len(), 4);
        assert_eq!(status.unstaged[0].path, "glob1.txt");
        assert!(matches!(status.unstaged[0].kind, StatusKind::Added));
        assert_eq!(status.unstaged[1].path, "glob[1].txt");
        assert!(matches!(status.unstaged[1].kind, StatusKind::Added));
        assert_eq!(status.unstaged[2].path, "new.txt");
        assert!(matches!(status.unstaged[2].kind, StatusKind::Added));
        assert_eq!(status.unstaged[3].path, "tracked.txt");
        assert!(matches!(status.unstaged[3].kind, StatusKind::Modified));

        let staged_diff = file_diff(&path, b"tracked.txt", StatusGroup::Staged).unwrap();
        assert!(matches!(staged_diff.kind, FileDiffKind::Text));
        assert!(staged_diff.patch.unwrap().contains("+staged"));
        let unstaged_diff = file_diff(&path, b"tracked.txt", StatusGroup::Unstaged).unwrap();
        let unstaged_patch = unstaged_diff.patch.unwrap();
        assert!(unstaged_patch.contains("-staged"));
        assert!(unstaged_patch.contains("+unstaged"));
        let phrase_diff = file_diff(&path, b"new.txt", StatusGroup::Unstaged).unwrap();
        assert!(matches!(phrase_diff.kind, FileDiffKind::Text));

        let untracked_diff = file_diff(&path, b"glob[1].txt", StatusGroup::Unstaged).unwrap();
        assert!(untracked_diff.patch.unwrap().contains("+literal"));
        let staged = stage_status_path(&path, b"glob[1].txt").unwrap();
        assert!(
            staged
                .staged
                .iter()
                .any(|entry| entry.path == "glob[1].txt")
        );
        assert!(!staged.staged.iter().any(|entry| entry.path == "glob1.txt"));
        assert!(
            staged
                .unstaged
                .iter()
                .any(|entry| entry.path == "glob1.txt")
        );
        assert!(staged.unstaged.iter().any(|entry| entry.path == "new.txt"));
        assert!(stage_status_path(&path, b"glob*").is_err());
        fs::write(path.join("binary.dat"), [0, 1, 2, 3]).unwrap();
        let binary_diff = file_diff(&path, b"binary.dat", StatusGroup::Unstaged).unwrap();
        assert!(matches!(binary_diff.kind, FileDiffKind::Binary));
        assert!(binary_diff.patch.is_none());
        assert!(file_diff(&path, b"glob*", StatusGroup::Unstaged).is_err());
        fs::write(
            path.join("large.txt"),
            vec![b'x'; MAX_GIT_STDOUT_BYTES + 32],
        )
        .unwrap();
        assert!(file_diff(&path, b"large.txt", StatusGroup::Unstaged).is_err());

        let unstaged = unstage_status_path(&path, b"glob[1].txt").unwrap();
        assert!(
            !unstaged
                .staged
                .iter()
                .any(|entry| entry.path == "glob[1].txt")
        );
        assert!(
            unstaged
                .unstaged
                .iter()
                .any(|entry| entry.path == "glob[1].txt")
        );
        assert!(unstage_status_path(&path, b"glob*").is_err());

        run_git(&["add", "tracked.txt"]);
        fs::remove_file(path.join("tracked.txt")).unwrap();
        let status = stage_status_path(&path, b"tracked.txt").unwrap();
        assert!(
            status
                .staged
                .iter()
                .any(|entry| { entry.path == "tracked.txt" && entry.kind == StatusKind::Removed })
        );

        let head = Command::new("git")
            .arg("-C")
            .arg(&path)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        assert!(head.status.success());
        let cacheinfo = format!(
            "160000,{},submodule",
            String::from_utf8_lossy(&head.stdout).trim()
        );
        run_git(&["update-index", "--add", "--cacheinfo", &cacheinfo]);
        assert!(is_gitlink(&path, b"submodule").unwrap());
        assert!(!is_gitlink(&path, b"submodule-other").unwrap());
        let _ = fs::remove_dir_all(&path);
    }

    #[test]
    fn unstages_a_new_file_without_a_head_and_keeps_worktree_content() {
        use std::{
            fs,
            process::Command,
            time::{SystemTime, UNIX_EPOCH},
        };

        let path = std::env::temp_dir().join(format!(
            "gitmiro-unborn-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        let run_git = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        run_git(&["init", "--quiet"]);
        fs::write(path.join("new.txt"), "staged\n").unwrap();
        run_git(&["add", "new.txt"]);
        fs::write(path.join("new.txt"), "keep this worktree content\n").unwrap();
        let staged_diff = file_diff(&path, b"new.txt", StatusGroup::Staged).unwrap();
        assert!(staged_diff.patch.unwrap().contains("+staged"));

        let status = unstage_status_path(&path, b"new.txt").unwrap();

        assert!(status.staged.is_empty());
        assert!(status.unstaged.iter().any(|entry| entry.path == "new.txt"));
        assert_eq!(
            fs::read_to_string(path.join("new.txt")).unwrap(),
            "keep this worktree content\n"
        );
        let _ = fs::remove_dir_all(&path);
    }

    #[test]
    fn resolves_conflicts_by_exact_requested_side() {
        use std::{
            fs,
            process::Command,
            time::{SystemTime, UNIX_EPOCH},
        };

        let path = std::env::temp_dir().join(format!(
            "gitmiro-conflict-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        let run_git = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        run_git(&["init", "--quiet"]);
        run_git(&["config", "user.name", "GitMiro Test"]);
        run_git(&["config", "user.email", "gitmiro-test@example.invalid"]);
        run_git(&["config", "commit.gpgsign", "false"]);
        run_git(&["config", "core.autocrlf", "false"]);
        run_git(&["config", "core.hooksPath", ".git/gitmiro-test-hooks"]);
        fs::create_dir(path.join(".git/gitmiro-test-hooks")).unwrap();
        fs::write(path.join("conflict[1].txt"), "base\n").unwrap();
        fs::write(path.join("delete.txt"), "base\n").unwrap();
        fs::write(path.join("invalid.txt"), b"base\n").unwrap();
        fs::write(path.join("large.txt"), b"base\n").unwrap();
        run_git(&["add", "--all"]);
        run_git(&["commit", "--quiet", "-m", "base"]);
        run_git(&["branch", "current"]);
        run_git(&["checkout", "--quiet", "-b", "incoming"]);
        fs::write(path.join("conflict[1].txt"), "incoming\n").unwrap();
        fs::write(path.join("invalid.txt"), b"incoming-\xff\n").unwrap();
        fs::write(
            path.join("large.txt"),
            "incoming\n".repeat(MAX_GIT_STDOUT_BYTES / 9 + 2),
        )
        .unwrap();
        fs::remove_file(path.join("delete.txt")).unwrap();
        run_git(&["add", "--all"]);
        run_git(&["commit", "--quiet", "-m", "incoming"]);
        run_git(&["checkout", "--quiet", "current"]);
        fs::write(path.join("conflict[1].txt"), "current\n").unwrap();
        fs::write(path.join("invalid.txt"), b"current-\xff\n").unwrap();
        fs::write(
            path.join("large.txt"),
            "current\n".repeat(MAX_GIT_STDOUT_BYTES / 8 + 2),
        )
        .unwrap();
        fs::write(path.join("delete.txt"), "current\n").unwrap();
        run_git(&["add", "--all"]);
        run_git(&["commit", "--quiet", "-m", "current"]);
        let merge = Command::new("git")
            .arg("-C")
            .arg(&path)
            .args(["merge", "--no-edit", "incoming"])
            .status()
            .unwrap();
        assert!(!merge.success(), "merge should create conflicts");

        let diff = file_diff(&path, b"conflict[1].txt", StatusGroup::Unstaged).unwrap();
        assert!(matches!(diff.kind, FileDiffKind::Conflict));
        assert_eq!(diff.current.unwrap().content.as_deref(), Some("current\n"));
        assert_eq!(
            diff.incoming.unwrap().content.as_deref(),
            Some("incoming\n")
        );
        let diff = file_diff(&path, b"delete.txt", StatusGroup::Unstaged).unwrap();
        assert!(matches!(
            diff.incoming.unwrap().kind,
            ConflictVersionKind::Deleted
        ));
        let diff = file_diff(&path, b"invalid.txt", StatusGroup::Unstaged).unwrap();
        let current = diff.current.unwrap();
        assert!(matches!(current.kind, ConflictVersionKind::Text));
        assert!(current.content.unwrap().contains('\u{fffd}'));
        let diff = file_diff(&path, b"large.txt", StatusGroup::Unstaged).unwrap();
        assert!(matches!(
            diff.current.unwrap().kind,
            ConflictVersionKind::TooLarge
        ));
        assert!(matches!(
            diff.incoming.unwrap().kind,
            ConflictVersionKind::TooLarge
        ));

        let status =
            resolve_conflict_side(&path, b"conflict[1].txt", ConflictSide::Current).unwrap();
        assert!(!status.unstaged.iter().any(|entry| {
            entry.kind == StatusKind::Conflicting && entry.path_bytes == b"conflict[1].txt"
        }));
        assert!(
            !status
                .staged
                .iter()
                .chain(&status.unstaged)
                .any(|entry| entry.path_bytes == b"conflict[1].txt")
        );
        assert_eq!(
            fs::read_to_string(path.join("conflict[1].txt")).unwrap(),
            "current\n"
        );

        let status = resolve_conflict_side(&path, b"delete.txt", ConflictSide::Incoming).unwrap();
        assert!(!status.unstaged.iter().any(|entry| {
            entry.kind == StatusKind::Conflicting && entry.path_bytes == b"delete.txt"
        }));
        assert!(
            status
                .staged
                .iter()
                .any(|entry| entry.path == "delete.txt" && entry.kind == StatusKind::Removed)
        );
        assert!(!path.join("delete.txt").exists());
        assert!(resolve_conflict_side(&path, b"conflict[1].txt", ConflictSide::Incoming).is_err());
        let _ = fs::remove_dir_all(&path);
    }

    #[test]
    fn reports_a_clear_error_for_a_directory_without_a_git_repository() {
        use std::{
            fs,
            time::{SystemTime, UNIX_EPOCH},
        };

        let path = std::env::temp_dir().join(format!(
            "gitmiro-not-a-repository-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();

        let result = repository_root(&path);

        let _ = fs::remove_dir_all(&path);
        let error = result.unwrap_err();
        assert_eq!(
            error,
            "Failed to open repository, could not find a repository in the specified path"
        );
    }

    #[cfg(windows)]
    #[test]
    fn drains_both_child_pipes_before_waiting_for_exit() {
        use std::process::{Command, Stdio};

        let child = Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[Console]::Out.Write('o' * 131072); [Console]::Error.Write('e' * 65536)",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let output = collect_output(child).unwrap();

        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 131072);
        assert_eq!(
            output.stderr.len(),
            MAX_GIT_STDERR_BYTES + b"\n[Git error output truncated]".len()
        );
    }

    #[test]
    fn parses_nul_delimited_git_log_records() {
        let output = format!(
            "abc123\0parent123 parent456\0subject\u{1f}value\0Pierre\0{date}\0\ndef456\0\0Second\0Ada\0{date}\0\n",
            date = "2026-11-04T10:00:00+01:00"
        );
        let commits = parse_commits(&output);

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].id, "abc123");
        assert_eq!(commits[0].parents, ["parent123", "parent456"]);
        assert_eq!(commits[0].subject, "subject\u{1f}value");
        assert_eq!(commits[0].author, "Pierre");
        assert_eq!(commits[0].date, "2026-11-04T10:00:00+01:00");
        assert_eq!(commits[1].id, "def456");
        assert_eq!(commits[1].author, "Ada");
    }

    #[test]
    fn lays_out_merge_lanes_and_preserves_lane_state_across_pages() {
        let commit = |id: &str, parents: &[&str]| Commit {
            id: id.to_owned(),
            subject: String::new(),
            author: String::new(),
            date: String::new(),
            parents: parents.iter().map(|parent| (*parent).to_owned()).collect(),
            lane: 0,
            passing_lanes: Vec::new(),
            merging_lanes: Vec::new(),
            forking_off_lanes: Vec::new(),
            child_count: 0,
            references: Vec::new(),
        };

        let first_page = page_commits(
            vec![commit("merge", &["left", "right"])],
            GraphState::default(),
        );

        assert_eq!(first_page.commits[0].lane, 0);
        assert_eq!(first_page.commits[0].merging_lanes, [1]);
        assert_eq!(
            first_page.graph_state.reserved_lanes.get(&0).unwrap(),
            "left"
        );
        assert_eq!(
            first_page.graph_state.reserved_lanes.get(&1).unwrap(),
            "right"
        );

        let second_page = page_commits(
            vec![commit("left", &["right"]), commit("right", &[])],
            first_page.graph_state,
        );

        assert_eq!(second_page.commits[0].passing_lanes, [1]);
        assert_eq!(second_page.commits[1].child_count, 2);
        assert_eq!(second_page.commits[1].forking_off_lanes, [1]);
    }

    #[test]
    fn paginates_commits_without_dropping_the_next_page_marker() {
        let commits = |count| {
            (0..count)
                .map(|id| Commit {
                    id: id.to_string(),
                    subject: String::new(),
                    author: String::new(),
                    date: String::new(),
                    parents: Vec::new(),
                    lane: 0,
                    passing_lanes: Vec::new(),
                    merging_lanes: Vec::new(),
                    forking_off_lanes: Vec::new(),
                    child_count: 0,
                    references: Vec::new(),
                })
                .collect()
        };

        let page = page_commits(commits(COMMIT_PAGE_SIZE + 1), GraphState::default());

        assert_eq!(page.commits.len(), COMMIT_PAGE_SIZE);
        assert!(page.has_more_commits);
        assert!(!page_commits(commits(COMMIT_PAGE_SIZE), GraphState::default()).has_more_commits);
    }

    #[test]
    fn pagination_only_accepts_full_commit_ids() {
        assert!(is_commit_id(&"a".repeat(40)));
        assert!(is_commit_id(&"a".repeat(64)));
        assert!(!is_commit_id("HEAD~1"));
        assert!(!is_commit_id(&"g".repeat(40)));
    }

    #[test]
    fn rejects_invalid_graph_cursors_before_git_is_called() {
        let mut state = GraphState::default();
        state.reserved_lanes.insert(usize::MAX, "a".repeat(40));
        assert_eq!(
            validate_graph_state(&state).unwrap_err(),
            "Invalid commit graph cursor"
        );

        state.reserved_lanes.clear();
        state.reserved_lanes.insert(0, "invalid".to_owned());
        assert_eq!(
            validate_graph_state(&state).unwrap_err(),
            "Invalid commit graph cursor"
        );
    }

    #[test]
    fn pagination_stays_on_the_head_captured_when_repository_opens() {
        use std::{
            fs,
            process::Command,
            time::{SystemTime, UNIX_EPOCH},
        };

        let path = std::env::temp_dir().join(format!(
            "gitmiro-pinned-history-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        let run_git = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success());
        };
        run_git(&["init", "--quiet"]);
        run_git(&["config", "user.name", "GitMiro Test"]);
        run_git(&["config", "user.email", "gitmiro-test@example.invalid"]);
        run_git(&["commit", "--allow-empty", "-m", "initial", "--quiet"]);
        let initial_head = history_head(&path).unwrap().unwrap();
        run_git(&["commit", "--allow-empty", "-m", "later", "--quiet"]);

        let refs = super::HistoryRefs {
            tips: vec![initial_head],
            references: Vec::new(),
        };
        let page = commit_page(&path, &refs, 0, GraphState::default()).unwrap();

        let _ = fs::remove_dir_all(&path);
        assert_eq!(page.commits.len(), 1);
        assert_eq!(page.commits[0].subject, "initial");
    }

    #[test]
    fn history_includes_unmerged_branches_and_peels_annotated_tags() {
        use std::{
            fs,
            process::Command,
            time::{SystemTime, UNIX_EPOCH},
        };

        let path = std::env::temp_dir().join(format!(
            "gitmiro-history-refs-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        let run_git = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        run_git(&["init", "--quiet", "--initial-branch=main"]);
        run_git(&["config", "user.name", "GitMiro Test"]);
        run_git(&["config", "user.email", "gitmiro-test@example.invalid"]);
        run_git(&["commit", "--allow-empty", "-m", "base", "--quiet"]);
        run_git(&["checkout", "--quiet", "-b", "feature"]);
        run_git(&["commit", "--allow-empty", "-m", "feature", "--quiet"]);
        run_git(&["tag", "-a", "feature-release", "-m", "feature release"]);
        run_git(&["checkout", "--quiet", "main"]);
        run_git(&["commit", "--allow-empty", "-m", "main", "--quiet"]);

        let head = history_head(&path).unwrap().unwrap();
        let refs = history_refs(&path, Some(&head)).unwrap();
        let page = commit_page(&path, &refs, 0, GraphState::default()).unwrap();
        let subjects = page
            .commits
            .iter()
            .map(|commit| commit.subject.as_str())
            .collect::<Vec<_>>();

        let _ = fs::remove_dir_all(&path);
        assert!(subjects.contains(&"main"));
        assert!(subjects.contains(&"feature"));
        assert!(subjects.contains(&"base"));
        assert_eq!(refs.tips.len(), 2);
        assert!(
            refs.references
                .iter()
                .any(|reference| reference.name == "feature-release"
                    && reference.kind == super::ReferenceKind::Tag)
        );
        assert!(
            page.commits
                .iter()
                .find(|commit| commit.subject == "feature")
                .unwrap()
                .references
                .iter()
                .any(|reference| reference.name == "feature"
                    && reference.kind == super::ReferenceKind::Local)
        );
    }

    #[test]
    fn unborn_branch_is_not_reported_as_detached_head() {
        use std::{
            fs,
            process::Command,
            time::{SystemTime, UNIX_EPOCH},
        };

        let path = std::env::temp_dir().join(format!(
            "gitmiro-unborn-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        let init = Command::new("git")
            .args(["init", "--quiet"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(init.success());

        let branch = current_branch(&path).unwrap();
        let _ = fs::remove_dir_all(&path);
        assert_ne!(branch, "Detached HEAD");
    }
}

#[cfg(test)]
mod regression_tests {
    //! GM-67 regression tests. Each one reproduces a defect on a throwaway repository.
    use super::{
        AppState, ConflictSide, GIT_TIMEOUT, MAX_GIT_STDOUT_BYTES, StatusGroup, StatusKind,
        collect_output_inner, commit_changes, commit_message_metadata, commit_page, file_diff,
        git_command, history_refs, repository_root, repository_status, resolve_conflict_side,
        write_commit_message,
    };
    use std::{
        fs,
        path::{Path, PathBuf},
        process::{Output, Stdio},
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct TempRepo {
        path: PathBuf,
    }

    impl TempRepo {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "gitmiro-gm67-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            let repo = Self { path };
            repo.git(&["init", "--quiet", "-b", "main"]);
            for (key, value) in [
                ("user.name", "GitMiro Test"),
                ("user.email", "gitmiro-test@example.invalid"),
                ("commit.gpgsign", "false"),
                ("core.autocrlf", "false"),
            ] {
                repo.git(&["config", key, value]);
            }
            repo
        }

        fn run(&self, args: &[&str]) -> Output {
            git_command()
                .arg("-C")
                .arg(&self.path)
                .args(args)
                .output()
                .unwrap()
        }

        fn git(&self, args: &[&str]) -> Output {
            let output = self.run(args);
            assert!(
                output.status.success(),
                "git {args:?}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            output
        }

        fn text(&self, args: &[&str]) -> String {
            String::from_utf8(self.git(args).stdout)
                .unwrap()
                .trim()
                .to_owned()
        }

        fn write(&self, name: &str, contents: &str) {
            fs::write(self.path.join(name), contents).unwrap();
        }

        fn commit_file(&self, name: &str, contents: &str, message: &str) {
            self.write(name, contents);
            self.git(&["add", name]);
            self.git(&["commit", "--quiet", "-m", message]);
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn commits_a_merge_whose_resolved_index_equals_head() {
        let repo = TempRepo::new();
        repo.commit_file("file.txt", "base\n", "base");
        repo.git(&["checkout", "--quiet", "-b", "side"]);
        repo.commit_file("file.txt", "changed\n", "side change");
        repo.git(&["checkout", "--quiet", "main"]);
        repo.commit_file("file.txt", "changed\n", "same change on main");
        repo.git(&["merge", "--no-commit", "--no-ff", "side"]);

        let status = repository_status(&repo.path).unwrap();
        assert!(status.staged.is_empty() && status.unstaged.is_empty());
        assert!(status.merging);

        commit_changes(&repo.path, "Merge side", false, None, false).unwrap();

        let parents = repo.text(&["rev-list", "--parents", "-n", "1", "HEAD"]);
        assert_eq!(parents.split(' ').count(), 3, "HEAD must be a merge commit");
        assert!(!repository_status(&repo.path).unwrap().merging);
    }

    #[test]
    fn a_clean_non_merge_state_still_refuses_an_empty_commit() {
        let repo = TempRepo::new();
        repo.commit_file("file.txt", "base\n", "base");

        assert_eq!(
            commit_changes(&repo.path, "nothing", false, None, false).err(),
            Some("Stage changes before committing".to_owned())
        );
    }

    #[test]
    fn reads_the_identity_from_included_config_files() {
        let repo = TempRepo::new();
        repo.git(&["config", "--unset", "user.name"]);
        repo.git(&["config", "--unset", "user.email"]);
        fs::write(
            repo.path.join(".git/identity.inc"),
            "[user]\n\tname = Included Author\n\temail = included@example.invalid\n",
        )
        .unwrap();
        repo.git(&["config", "include.path", "identity.inc"]);

        let identity = commit_message_metadata(&repo.path).unwrap().local_identity;

        assert_eq!(identity.name.as_deref(), Some("Included Author"));
        assert_eq!(identity.email.as_deref(), Some("included@example.invalid"));
    }

    #[test]
    fn amending_does_not_duplicate_the_sign_off() {
        let repo = TempRepo::new();
        repo.commit_file("file.txt", "base\n", "base");
        fs::write(
            repo.path.join(".git/gitnuro"),
            "[signoff]\n\tenabled = true\n",
        )
        .unwrap();
        repo.write("file.txt", "staged\n");
        repo.git(&["add", "file.txt"]);
        commit_changes(&repo.path, "feature", false, None, false).unwrap();
        let previous = repo.text(&["log", "-1", "--format=%B"]);
        assert_eq!(previous.matches("Signed-off-by").count(), 1);

        commit_changes(&repo.path, &previous, true, None, false).unwrap();

        let amended = repo.text(&["log", "-1", "--format=%B"]);
        assert_eq!(amended.matches("Signed-off-by").count(), 1, "{amended}");
    }

    #[test]
    fn a_non_utf8_commit_draft_does_not_block_opening() {
        let repo = TempRepo::new();
        repo.commit_file("file.txt", "base\n", "base");
        // Latin-1 "caf\xe9" as written by another tool.
        fs::write(repo.path.join(".git/COMMIT_EDITMSG"), b"WIP caf\xe9\n").unwrap();

        let metadata = commit_message_metadata(&repo.path).unwrap();

        assert!(
            metadata.draft.starts_with("WIP caf"),
            "{:?}",
            metadata.draft
        );
    }

    #[test]
    fn a_terminal_commit_does_not_leave_a_stale_draft() {
        let repo = TempRepo::new();
        repo.commit_file("file.txt", "base\n", "committed in a terminal");
        assert_eq!(commit_message_metadata(&repo.path).unwrap().draft, "");

        write_commit_message(&repo.path, "work in progress").unwrap();
        assert_eq!(
            commit_message_metadata(&repo.path).unwrap().draft,
            "work in progress"
        );
    }

    #[test]
    fn resolves_a_both_deleted_conflict() {
        let repo = TempRepo::new();
        let body = (0..20).map(|n| format!("line {n}\n")).collect::<String>();
        repo.commit_file("d.txt", &body, "base");
        repo.git(&["checkout", "--quiet", "-b", "left"]);
        repo.git(&["mv", "d.txt", "left.txt"]);
        repo.git(&["commit", "--quiet", "-m", "rename left"]);
        repo.git(&["checkout", "--quiet", "-b", "right", "main"]);
        repo.git(&["mv", "d.txt", "right.txt"]);
        repo.git(&["commit", "--quiet", "-m", "rename right"]);
        repo.git(&["checkout", "--quiet", "left"]);
        assert!(!repo.run(&["merge", "right"]).status.success());

        let status = repository_status(&repo.path).unwrap();
        let both_deleted = status
            .unstaged
            .iter()
            .find(|entry| entry.path == "d.txt")
            .expect("d.txt must be reported as a conflict");
        assert_eq!(both_deleted.kind, StatusKind::Conflicting);

        let diff = file_diff(&repo.path, b"d.txt", StatusGroup::Unstaged).unwrap();
        assert!(diff.current.is_some() && diff.incoming.is_some());

        let after = resolve_conflict_side(&repo.path, b"d.txt", ConflictSide::Current).unwrap();
        assert!(after.unstaged.iter().all(|entry| entry.path != "d.txt"));
        assert!(
            repo.text(&["ls-files", "--unmerged", "--", "d.txt"])
                .is_empty()
        );
    }

    #[test]
    fn opens_a_repository_with_thousands_of_long_references() {
        let repo = TempRepo::new();
        repo.commit_file("file.txt", "base\n", "base");
        let head = repo.text(&["rev-parse", "HEAD"]);
        let updates = (0..3500)
            .map(|n| format!("create refs/tags/release-candidate-{n:0>90} {head}\n"))
            .collect::<String>();
        let mut child = git_command()
            .arg("-C")
            .arg(&repo.path)
            .args(["update-ref", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let output = collect_output_inner(
            &mut child,
            Some(updates.as_bytes()),
            false,
            false,
            MAX_GIT_STDOUT_BYTES,
            GIT_TIMEOUT,
        )
        .unwrap()
        .0;
        assert!(output.status.success());

        let refs = history_refs(&repo.path, Some(&head)).unwrap();
        assert_eq!(refs.references.len(), 3501, "3500 tags plus main");
        let page = commit_page(&repo.path, &refs, 0, Default::default()).unwrap();
        assert_eq!(page.commits.len(), 1);
        assert_eq!(page.commits[0].references.len(), 3501);
    }

    #[test]
    fn stops_a_git_process_that_never_finishes() {
        let repo = TempRepo::new();
        // Waits for stdin, which stays open: stands in for a credential prompt or a stuck hook.
        let mut child = git_command()
            .arg("-C")
            .arg(&repo.path)
            .args(["hash-object", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let error = collect_output_inner(
            &mut child,
            None,
            false,
            false,
            MAX_GIT_STDOUT_BYTES,
            Duration::from_millis(300),
        )
        .unwrap_err();

        assert!(error.contains("did not finish"), "{error}");
    }

    #[test]
    fn never_runs_a_repository_configured_fsmonitor_command() {
        let repo = TempRepo::new();
        repo.commit_file("file.txt", "base\n", "base");
        repo.git(&[
            "config",
            "core.fsmonitor",
            "echo ran > fsmonitor-marker.txt",
        ]);
        let marker = repo.path.join("fsmonitor-marker.txt");

        repository_status(&repo.path).unwrap();

        assert!(!marker.exists(), "core.fsmonitor command was executed");
    }

    #[test]
    fn only_the_opened_repository_is_accepted_by_commands() {
        let opened = TempRepo::new();
        let other = TempRepo::new();
        let state = AppState::default();
        let opened_path = opened.path.to_str().unwrap();
        let other_path = other.path.to_str().unwrap();

        assert!(state.open_root(opened_path).is_err(), "nothing is open yet");
        state.set_open_root(repository_root(Path::new(opened_path)).unwrap());

        assert!(state.open_root(opened_path).is_ok());
        assert!(state.open_root(other_path).is_err());
        assert!(state.open_root("Z:/does/not/exist").is_err());
    }
}
