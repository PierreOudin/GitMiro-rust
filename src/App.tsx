import "./App.css";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { type CSSProperties, type UIEvent, useEffect, useMemo, useRef, useState } from "react";
import { reachableCommitIds } from "./history-filter";
import { isNearEnd } from "./scroll";

type GraphState = { reserved_lanes: Record<string, string> };

type RepositorySummary = {
  path: string;
  name: string;
  branch: string;
  history_head: string | null;
  history_refs: { tips: string[]; references: GitReference[] };
  commit: CommitMetadata;
  commits: Commit[];
  has_more_commits: boolean;
  graph_state: GraphState;
  status: RepositoryStatus;
};

type CommitIdentity = { name: string | null; email: string | null };
type CommitMetadata = {
  draft: string;
  previous_message: string | null;
  local_identity: CommitIdentity;
  global_identity: CommitIdentity;
};

type RepositoryStatus = { staged: StatusEntry[]; unstaged: StatusEntry[]; truncated: boolean; merging: boolean };
type StatusEntry = { path: string; path_bytes: number[]; kind: "added" | "modified" | "removed" | "conflicting" };
type StatusGroup = "staged" | "unstaged";
type ConflictVersion = { kind: "text" | "binary" | "submodule" | "deleted" | "tooLarge"; content: string | null };
type FileDiff = {
  kind: "text" | "binary" | "submodule" | "conflict";
  patch: string | null;
  current: ConflictVersion | null;
  incoming: ConflictVersion | null;
};

type Commit = {
  id: string;
  subject: string;
  author: string;
  date: string;
  parents: string[];
  lane: number;
  passing_lanes: number[];
  merging_lanes: number[];
  forking_off_lanes: number[];
  child_count: number;
  references: GitReference[];
};

type GitReference = { id: string; name: string; kind: "local" | "remote" | "tag" };

type CommitPage = Pick<RepositorySummary, "commits" | "has_more_commits" | "graph_state">;

const GRAPH_COLORS = ["#42a5f5", "#ef5350", "#78909c", "#ff7043", "#66bb6a", "#ec407a"];
const GRAPH_LANE_WIDTH = 18;
const COMMIT_ROW_HEIGHT = 50;
const REFERENCE_GROUPS: Array<{ kind: GitReference["kind"]; label: string }> = [
  { kind: "local", label: "Local branches" },
  { kind: "remote", label: "Remotes" },
  { kind: "tag", label: "Tags" },
];

function referenceKey(reference: GitReference) {
  return `${reference.kind}:${reference.name}`;
}

function samePathBytes(left: number[], right: number[]) {
  return left.length === right.length && left.every((byte, index) => byte === right[index]);
}

function CommitGraph({ commit }: { commit: Commit }) {
  const laneX = (lane: number) => GRAPH_LANE_WIDTH * (lane + 1);
  const centerY = COMMIT_ROW_HEIGHT / 2;
  const color = (lane: number) => GRAPH_COLORS[lane % GRAPH_COLORS.length];

  return (
    <svg className="commit-graph" aria-hidden="true">
      {commit.child_count > 0 && (
        <line x1={laneX(commit.lane)} y1="0" x2={laneX(commit.lane)} y2={centerY} stroke={color(commit.lane)} />
      )}
      {commit.passing_lanes.map((lane) => (
        <line key={`pass-${lane}`} x1={laneX(lane)} y1="0" x2={laneX(lane)} y2={COMMIT_ROW_HEIGHT} stroke={color(lane)} />
      ))}
      {commit.parents.length > 0 && (
        <line x1={laneX(commit.lane)} y1={centerY} x2={laneX(commit.lane)} y2={COMMIT_ROW_HEIGHT} stroke={color(commit.lane)} />
      )}
      {commit.forking_off_lanes.map((lane) => (
        <path
          key={`fork-${lane}`}
          d={`M ${laneX(commit.lane)} ${centerY} C ${laneX(commit.lane)} ${centerY - 14}, ${laneX(lane)} 14, ${laneX(lane)} 0`}
          stroke={color(lane)}
        />
      ))}
      {commit.merging_lanes.map((lane) => (
        <path
          key={`merge-${lane}`}
          d={`M ${laneX(commit.lane)} ${centerY} C ${laneX(commit.lane)} ${centerY + 14}, ${laneX(lane)} ${COMMIT_ROW_HEIGHT - 14}, ${laneX(lane)} ${COMMIT_ROW_HEIGHT}`}
          stroke={color(lane)}
        />
      ))}
      {commit.parents.length > 1 ? (
        <rect x={laneX(commit.lane) - 5} y={centerY - 5} width="10" height="10" rx="3" fill={color(commit.lane)} />
      ) : (
        <circle cx={laneX(commit.lane)} cy={centerY} r="5" fill={color(commit.lane)} />
      )}
    </svg>
  );
}

const RECENT_REPOSITORIES_KEY = "gitmiro.recentRepositories";

function readRecentRepositories(): string[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(RECENT_REPOSITORIES_KEY) ?? "[]");
    return Array.isArray(value) && value.every((path) => typeof path === "string") ? value : [];
  } catch {
    return [];
  }
}

// Stable, so React calls it only when the dialog is mounted.
function showModal(dialog: HTMLDialogElement | null) {
  dialog?.showModal();
}

function App() {
  const [repository, setRepository] = useState<RepositorySummary | null>(null);
  const [recentRepositories, setRecentRepositories] = useState(readRecentRepositories);
  const [recentFilter, setRecentFilter] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [opening, setOpening] = useState(false);
  const [historyFilter, setHistoryFilter] = useState("");
  const [loadingMore, setLoadingMore] = useState(false);
  const [changingStatus, setChangingStatus] = useState(false);
  const [selectedDiff, setSelectedDiff] = useState<{ path: string; pathBytes: number[]; group: StatusGroup } | null>(null);
  const [fileDiff, setFileDiff] = useState<FileDiff | null>(null);
  const [diffLoading, setDiffLoading] = useState(false);
  const [diffError, setDiffError] = useState<string | null>(null);
  const [commitMessage, setCommitMessage] = useState("");
  const [isAmend, setIsAmend] = useState(false);
  // A merge in progress cannot be amended (Git refuses), whatever the box was ticked to before the merge began.
  const amending = isAmend && !repository?.status.merging;
  const [committing, setCommitting] = useState(false);
  const [commitError, setCommitError] = useState<string | null>(null);
  const [identityPrompt, setIdentityPrompt] = useState<{ name: string; email: string; save: boolean } | null>(null);
  const [identityError, setIdentityError] = useState<string | null>(null);
  const [openGeneration, setOpenGeneration] = useState(0);
  // What the last operation is doing or did, announced politely; errors use their own role="alert" messages.
  const [activity, setActivity] = useState("");
  const selectedDiffRef = useRef<typeof selectedDiff>(null);
  const openingRef = useRef(false);
  const loadingMoreRef = useRef(false);
  const loadMoreFailedRef = useRef(false);
  const repositoryPathRef = useRef<string | null>(null);
  const identityOpenerRef = useRef<HTMLElement | null>(null);
  const focusTargetRef = useRef<(() => HTMLElement | null | undefined) | null>(null);
  const changingStatusRef = useRef(false);
  const committingRef = useRef(false);
  const commitPersistTimerRef = useRef<number | null>(null);
  const diffRequestRef = useRef(0);
  const historyRequestRef = useRef(0);
  const statusRefreshRef = useRef(0);
  const repositoryHeadingRef = useRef<HTMLHeadingElement>(null);
  const recentRepositoryRefs = useRef<Array<HTMLButtonElement | null>>([]);

  repositoryPathRef.current = repository?.path ?? null;

  // Only when a repository is opened: status refreshes replace `repository` and must not move the focus.
  useEffect(() => {
    if (openGeneration > 0) repositoryHeadingRef.current?.focus();
  }, [openGeneration]);

  useEffect(() => {
    if (!repository) return;
    let active = true;
    let unlisten: (() => void) | undefined;
    void getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (!focused || changingStatusRef.current || committingRef.current) return;
      const refreshId = ++statusRefreshRef.current;
      void invoke<RepositoryStatus>("refresh_repository_status", { path: repository.path })
        .then((status) => {
          if (active && refreshId === statusRefreshRef.current) {
            setStatusError(null);
            setRepository((current) => current?.path === repository.path ? { ...current, status } : current);
          }
        })
        .catch((error) => {
          if (active && refreshId === statusRefreshRef.current) setStatusError(String(error));
        });
    }).then((stop) => {
      if (active) unlisten = stop;
      else stop();
    });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [repository?.path]);

  // The list scrolls inside .commit-history, not the window. After a failed page the manual button retries.
  function loadMoreWhenScrolledToEnd(event: UIEvent<HTMLDivElement>) {
    if (!loadMoreFailedRef.current && isNearEnd(event.currentTarget)) void loadMoreCommits();
  }

  // An action that disables or removes the focused control drops the focus to <body>. Once nothing is busy, move it
  // where that action asked (the message box if that control is gone), unless the user has moved it meanwhile.
  useEffect(() => {
    const target = focusTargetRef.current;
    if (!target || opening || committing || changingStatus) return;
    focusTargetRef.current = null;
    if (document.activeElement && document.activeElement !== document.body) return;
    [target(), document.getElementById("commit-message")]
      .find((element) => element?.isConnected && !element.matches(":disabled"))
      ?.focus();
  });

  // Handled on the document so they work wherever the focus is, <body> included. Only refs and setters are used, so
  // the functions captured by the first render stay valid.
  useEffect(() => {
    function handleKey(event: globalThis.KeyboardEvent) {
      if (event.key === "Escape" && document.querySelector("dialog[open]")) {
        // Prevented here, Escape never closes the dialog natively: React state alone decides, and a commit started
        // from the dialog cannot be stopped safely, so the dialog stays until it ends.
        event.preventDefault();
        if (!committingRef.current) closeIdentityPrompt();
      } else if (event.key === "Escape" && selectedDiffRef.current) {
        event.preventDefault();
        backToHistory();
      } else if (event.key === "F6") {
        // F6 / Shift+F6 move between the reference, history (or diff) and status panes, as in Windows applications.
        const panes = [...document.querySelectorAll<HTMLElement>(".reference-sidebar, .history-panel, .status-column")];
        if (!panes.length) return;
        event.preventDefault();
        const step = event.shiftKey ? -1 : 1;
        const current = panes.findIndex((pane) => pane.contains(document.activeElement));
        const start = current < 0 ? (event.shiftKey ? 0 : panes.length - 1) : current;
        for (let offset = 1; offset <= panes.length; offset++) {
          const pane = panes[(start + step * offset + panes.length * offset) % panes.length];
          const target = pane.querySelector<HTMLElement>(".reference-item-selected")
            ?? pane.querySelector<HTMLElement>("button:not(:disabled), select, textarea:not(:disabled)");
          if (target) return target.focus(); // skip a pane with nothing to focus (no references yet)
        }
      }
    }
    document.addEventListener("keydown", handleKey);
    return () => document.removeEventListener("keydown", handleKey);
  }, []);

  function closeIdentityPrompt() {
    setIdentityPrompt(null);
    const opener = identityOpenerRef.current;
    focusTargetRef.current = () => opener;
  }

  // A page still loading belongs to the history being replaced, and an earlier page failure no longer applies.
  function resetHistoryPaging() {
    historyRequestRef.current++;
    loadingMoreRef.current = false;
    loadMoreFailedRef.current = false;
    setLoadingMore(false);
  }

  async function openRepository(path?: string) {
    if (openingRef.current || committingRef.current) return;
    openingRef.current = true;
    setOpening(true);
    setError(null);
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    focusTargetRef.current = () => opener; // on success the repository title takes the focus instead
    try {
      const chosen = path ?? await open({ directory: true, multiple: false });
      if (!chosen || Array.isArray(chosen)) return;
      setActivity(`Opening ${chosen}…`);
      await openRepositoryPath(chosen);
    } catch (error) {
      setError(String(error));
      setActivity("");
    } finally {
      openingRef.current = false;
      setOpening(false);
    }
  }

  async function openRepositoryPath(path: string) {
    const opened = await invoke<RepositorySummary>("open_repository", { path });
    setActivity(`Opened ${opened.name} on ${opened.branch}`);
    resetHistoryPaging();
    setHistoryFilter("");
    statusRefreshRef.current++;
    setStatusError(null);
    diffRequestRef.current++;
    if (commitPersistTimerRef.current !== null) {
      window.clearTimeout(commitPersistTimerRef.current);
      commitPersistTimerRef.current = null;
    }
    setCommitMessage(opened.commit.draft);
    setIsAmend(false);
    setCommitError(null);
    setIdentityPrompt(null);
    selectedDiffRef.current = null;
    setSelectedDiff(null);
    setFileDiff(null);
    setDiffError(null);
    setRepository(opened);
    setOpenGeneration((generation) => generation + 1);
    setRecentRepositories((current) => [opened.path, ...current.filter((recent) => recent !== opened.path)]);
  }

  function removeRecentRepository(path: string, index: number) {
    setRecentRepositories((current) => current.filter((recent) => recent !== path));
    focusTargetRef.current = () =>
      recentRepositoryRefs.current[index] ?? recentRepositoryRefs.current[index - 1] ?? document.getElementById("recent-filter");
  }

  function changeCommitMessage(message: string) {
    setCommitMessage(message);
    setCommitError(null);
    const targetPath = repository?.path;
    if (!targetPath) return;
    if (commitPersistTimerRef.current !== null) window.clearTimeout(commitPersistTimerRef.current);
    commitPersistTimerRef.current = window.setTimeout(() => {
      commitPersistTimerRef.current = null;
      void invoke("persist_commit_message", { path: targetPath, message }).catch((error) => {
        setCommitError(String(error));
      });
    }, 1000);
  }

  function identityNeedsInput(metadata: CommitMetadata) {
    const name = metadata.local_identity.name?.trim() || metadata.global_identity.name?.trim();
    const email = metadata.local_identity.email?.trim() || metadata.global_identity.email?.trim();
    return !name || !email;
  }

  async function commitChanges(identity?: { name: string; email: string }, persistIdentity = false) {
    if (!repository || committingRef.current || changingStatusRef.current) return;
    // A merge can be concluded with nothing staged (its result may equal HEAD).
    if (!commitMessage.length || (!amending && repository.status.staged.length === 0 && !repository.status.merging)) return;
    if (amending && !repository.history_head) return;
    if (!identity && identityNeedsInput(repository.commit)) {
      identityOpenerRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      setIdentityError(null);
      setIdentityPrompt({
        name: repository.commit.local_identity.name ?? repository.commit.global_identity.name ?? "",
        email: repository.commit.local_identity.email ?? repository.commit.global_identity.email ?? "",
        save: false,
      });
      return;
    }

    committingRef.current = true;
    setCommitting(true);
    setCommitError(null);
    setIdentityError(null);
    setActivity(amending ? "Amending the last commit…" : "Committing…");
    focusTargetRef.current = () => document.getElementById("commit-message"); // disabled while committing
    const targetPath = repository.path;
    statusRefreshRef.current++; // a status read that started before the commit would show the old index
    if (commitPersistTimerRef.current !== null) {
      window.clearTimeout(commitPersistTimerRef.current);
      commitPersistTimerRef.current = null;
    }
    try {
      await invoke("persist_commit_message", { path: targetPath, message: commitMessage });
      const updated = await invoke<RepositorySummary>("commit_repository", {
        path: targetPath,
        message: commitMessage,
        amend: amending,
        identity: identity ?? null,
        persistIdentity,
      });
      // Opening another repository is blocked while committing; still never let a late result replace the visible one.
      if (repositoryPathRef.current !== targetPath) return;
      setRepository(updated);
      setActivity(`${amending ? "Amended" : "Committed"} ${updated.history_head?.slice(0, 7) ?? ""} on ${updated.branch}`);
      // The history and status were replaced: earlier errors no longer apply either.
      resetHistoryPaging();
      setError(null);
      setStatusError(null);
      setCommitMessage(updated.commit.draft);
      setIsAmend(false);
      setIdentityPrompt(null);
      const selection = selectedDiffRef.current;
      if (selection) {
        const entry = updated.status[selection.group].find((candidate) => samePathBytes(candidate.path_bytes, selection.pathBytes));
        if (entry) void viewFileDiff(entry, selection.group);
        else closeFileDiff();
      }
    } catch (error) {
      const message = String(error);
      setCommitError(message);
      setActivity("");
      if (identity) {
        setIdentityError(message); // the dialog hides the composer where the error is otherwise shown
        document.getElementById("author-name")?.focus(); // Continue is about to be disabled; keep the focus in the dialog
      }
    } finally {
      committingRef.current = false;
      setCommitting(false);
    }
  }

  useEffect(() => {
    try {
      localStorage.setItem(RECENT_REPOSITORIES_KEY, JSON.stringify(recentRepositories));
    } catch {
      // Recent paths are a convenience; unavailable storage must not block opening a repository.
    }
  }, [recentRepositories]);

  const filteredRecentRepositories = recentRepositories.filter((path) =>
    path.toLocaleLowerCase().includes(recentFilter.toLocaleLowerCase()),
  );

  const branchReference = repository?.history_refs.references.find(
    (reference) => referenceKey(reference) === historyFilter,
  );
  const reachableCommits = useMemo(() => {
    if (!repository || !historyFilter || !branchReference) return null;
    return reachableCommitIds(repository.commits, branchReference.id);
  }, [repository?.commits, branchReference, historyFilter]);

  async function loadMoreCommits() {
    if (!repository || !repository.has_more_commits || loadingMoreRef.current) return;
    const requestId = historyRequestRef.current;
    loadingMoreRef.current = true;
    setLoadingMore(true);
    setError(null);
    setActivity("Loading older commits…");
    try {
      const page = await invoke<CommitPage>("load_more_commits", {
        path: repository.path,
        historyRefs: repository.history_refs,
        skip: repository.commits.length,
        graphState: repository.graph_state,
      });
      if (requestId === historyRequestRef.current) {
        setActivity(`Loaded ${page.commits.length} older commits`);
        setRepository((current) => current?.path === repository.path && current.history_head === repository.history_head
          ? {
            ...current,
            commits: [...current.commits, ...page.commits],
            has_more_commits: page.has_more_commits,
            graph_state: page.graph_state,
          }
          : current);
      }
    } catch (error) {
      if (requestId === historyRequestRef.current) {
        loadMoreFailedRef.current = true; // scrolling must not hammer Git; the button retries
        setError(String(error));
        setActivity("");
      }
    } finally {
      if (requestId === historyRequestRef.current) {
        loadingMoreRef.current = false;
        setLoadingMore(false);
      }
    }
  }

  async function viewFileDiff(entry: StatusEntry, group: StatusGroup) {
    if (!repository) return;
    const targetPath = repository.path;
    const requestId = ++diffRequestRef.current;
    const selection = { path: entry.path, pathBytes: entry.path_bytes, group };
    selectedDiffRef.current = selection;
    setSelectedDiff(selection);
    setFileDiff(null);
    setDiffError(null);
    setDiffLoading(true);
    try {
      const diff = await invoke<FileDiff>("get_file_diff", {
        path: targetPath,
        pathBytes: entry.path_bytes,
        group,
      });
      if (requestId === diffRequestRef.current) setFileDiff(diff);
    } catch (error) {
      if (requestId === diffRequestRef.current) setDiffError(String(error));
    } finally {
      if (requestId === diffRequestRef.current) setDiffLoading(false);
    }
  }

  function closeFileDiff() {
    diffRequestRef.current++;
    selectedDiffRef.current = null;
    setSelectedDiff(null);
    setFileDiff(null);
    setDiffError(null);
    setDiffLoading(false);
  }

  function backToHistory() {
    const selection = selectedDiffRef.current;
    closeFileDiff();
    focusTargetRef.current = () =>
      (selection && document.getElementById(`diff-${selection.group}-${selection.pathBytes.join("-")}`))
      ?? document.getElementById("history-branch-filter");
  }

  async function changeStatusEntry(entry: StatusEntry, action: "stage" | "unstage") {
    if (!repository || changingStatusRef.current || committingRef.current) return;
    const targetPath = repository.path;
    changingStatusRef.current = true;
    setChangingStatus(true);
    const requestId = ++statusRefreshRef.current;
    setStatusError(null);
    setActivity(`${action === "stage" ? "Staging" : "Unstaging"} ${entry.path}…`);
    // The entry leaves its list: keep the focus on the same row of that list, which now holds the next entry.
    const group = action === "stage" ? "unstaged" : "staged";
    const index = repository.status[group].indexOf(entry);
    focusTargetRef.current = () => {
      const buttons = document.querySelectorAll<HTMLElement>(`button[data-group="${group}"]`);
      return buttons[Math.min(index, buttons.length - 1)];
    };
    try {
      const status = await invoke<RepositoryStatus>(`${action}_status_entry`, {
        path: targetPath,
        pathBytes: entry.path_bytes,
      });
      if (requestId === statusRefreshRef.current) {
        setActivity(`${action === "stage" ? "Staged" : "Unstaged"} ${entry.path}`);
        setRepository((current) => current?.path === targetPath ? { ...current, status } : current);
        const currentSelection = selectedDiffRef.current;
        if (currentSelection && samePathBytes(currentSelection.pathBytes, entry.path_bytes)) {
          const nextGroup = action === "stage" ? "staged" : "unstaged";
          const updatedEntry = status[nextGroup].find((candidate) => samePathBytes(candidate.path_bytes, entry.path_bytes));
          if (updatedEntry) void viewFileDiff(updatedEntry, nextGroup);
          else closeFileDiff();
        }
      }
    } catch (error) {
      if (requestId === statusRefreshRef.current) {
        setStatusError(String(error));
        setActivity("");
      }
    } finally {
      changingStatusRef.current = false;
      setChangingStatus(false);
    }
  }

  async function resolveConflict(pathBytes: number[], side: "current" | "incoming") {
    if (!repository || changingStatusRef.current || committingRef.current) return;
    const targetPath = repository.path;
    changingStatusRef.current = true;
    setChangingStatus(true);
    const requestId = ++statusRefreshRef.current;
    setStatusError(null);
    setActivity("Resolving the conflict…");
    focusTargetRef.current = () => document.getElementById("commit-message"); // the Keep buttons are replaced
    try {
      const status = await invoke<RepositoryStatus>("resolve_conflict_file", {
        path: targetPath,
        pathBytes,
        side,
      });
      if (requestId === statusRefreshRef.current) {
        setActivity(`Conflict resolved with the ${side} side and staged`);
        setRepository((current) => current?.path === targetPath ? { ...current, status } : current);
        const currentSelection = selectedDiffRef.current;
        if (currentSelection && samePathBytes(currentSelection.pathBytes, pathBytes)) {
          const resolvedEntry = status.staged.find((candidate) => samePathBytes(candidate.path_bytes, pathBytes));
          if (resolvedEntry) void viewFileDiff(resolvedEntry, "staged");
          else closeFileDiff();
        }
      }
    } catch (error) {
      if (requestId === statusRefreshRef.current) {
        setStatusError(String(error));
        setActivity("");
      }
    } finally {
      changingStatusRef.current = false;
      setChangingStatus(false);
    }
  }

  return (
    <main className="app-shell" aria-label="GitMiro">
      <header className="titlebar" data-tauri-drag-region>
        <span className="wordmark">GitMiro</span>
        <p className="activity" role="status">{activity}</p>
      </header>
      {repository ? (
        <section className="repository-view">
          <header className="repository-toolbar">
            <div className="repository-heading">
              <div>
                <h1 ref={repositoryHeadingRef} tabIndex={-1}>{repository.name}</h1>
                <p className="repository-path">{repository.path}</p>
              </div>
              <span className="branch-label">{repository.branch}</span>
            </div>
            <button className="button button-secondary" onClick={() => void openRepository()} disabled={opening || changingStatus || committing}>
              {opening ? "Opening…" : "Open another"}
            </button>
          </header>
          {error && <p className="error-message repository-error" role="alert">{error}</p>}
          <div className="repository-layout">
            <aside className="reference-sidebar" aria-label="Repository references">
              {REFERENCE_GROUPS.map(({ kind, label }) => (
                <section className="reference-group" aria-label={label} key={kind}>
                  <h2>{label}</h2>
                  <ul>
                    {repository.history_refs.references
                      .filter((reference) => reference.kind === kind)
                      .map((reference) => {
                        const key = referenceKey(reference);
                        return (
                          <li key={key}>
                            <button
                              className={key === historyFilter ? "reference-item reference-item-selected" : "reference-item"}
                              aria-pressed={key === historyFilter}
                              onClick={() => setHistoryFilter(historyFilter === key ? "" : key)}
                            >
                              <span className="reference-dot" aria-hidden="true" />
                              <span>{reference.name}</span>
                            </button>
                          </li>
                        );
                      })}
                  </ul>
                </section>
              ))}
            </aside>
            <section className="history-panel" aria-label="Commit history">
              {selectedDiff ? (
                <div className="file-diff-view" aria-label="File diff">
                  <header className="file-diff-heading">
                    <button className="button button-secondary" onClick={backToHistory}>Back to history</button>
                    <div>
                      <h2 title={selectedDiff.path}>{selectedDiff.path}</h2>
                      <span>{selectedDiff.group === "staged" ? "Staged changes" : "Unstaged changes"}</span>
                    </div>
                  </header>
                  {diffLoading && <p className="muted" role="status">Loading diff…</p>}
                  {diffError && <p className="error-message" role="alert">{diffError}</p>}
                  {fileDiff?.kind === "binary" && <p className="muted" role="status">Binary file preview is not available.</p>}
                  {fileDiff?.kind === "conflict" && (
                    <>
                      <p className="muted" role="status">Choose a complete side to resolve and stage this file.</p>
                      <div className="conflict-preview">
                        {(["current", "incoming"] as const).map((side) => {
                          const version = fileDiff[side];
                          if (!version) return null;
                          const label = side === "current" ? "Current" : "Incoming";
                          return (
                            <section className="conflict-version" aria-label={`${label} side`} key={side}>
                              <header>
                                <h3>{label}</h3>
                                <button
                                  className="button button-secondary"
                                  type="button"
                                  onClick={() => void resolveConflict(selectedDiff.pathBytes, side)}
                                  disabled={changingStatus || committing}
                                >
                                  {version.kind === "deleted" ? `Keep ${side} (delete)` : `Keep ${side}`}
                                </button>
                              </header>
                              {version.kind === "submodule" ? (
                                <p className="muted" role="status">Submodule commit: {version.content}</p>
                              ) : version.content !== null ? (
                                <pre>{version.content}</pre>
                              ) : (
                                <p className="muted" role="status">
                                  {version.kind === "deleted" ? "File is deleted on this side."
                                    : version.kind === "tooLarge" ? "Side exceeds the 512 KiB preview limit."
                                      : "Binary side preview is unavailable."}
                                </p>
                              )}
                            </section>
                          );
                        })}
                      </div>
                    </>
                  )}
                  {fileDiff?.kind === "submodule" && <p className="muted" role="status">Submodule change</p>}
                  {fileDiff?.patch ? (
                    <pre className="file-diff-patch">{fileDiff.patch}</pre>
                  ) : fileDiff && fileDiff.kind !== "binary" && fileDiff.kind !== "conflict" ? (
                    <p className="muted">No diff available.</p>
                  ) : null}
                </div>
              ) : (
                <>
                  <div className="commit-history-heading">
                    <h2>Commit history</h2>
                    <label htmlFor="history-branch-filter">Reference</label>
                    <select
                      id="history-branch-filter"
                      value={historyFilter}
                      onChange={(event) => setHistoryFilter(event.target.value)}
                    >
                      <option value="">All references</option>
                      {REFERENCE_GROUPS.map(({ kind, label }) => (
                        <optgroup label={label} key={kind}>
                          {repository.history_refs.references
                            .filter((reference) => reference.kind === kind)
                            .map((reference) => {
                              const key = referenceKey(reference);
                              return <option value={key} key={key}>{reference.name}</option>;
                            })}
                        </optgroup>
                      ))}
                    </select>
                  </div>
                  {repository.commits.length ? (
                    <div className="commit-history" onScroll={loadMoreWhenScrolledToEnd} style={{ "--graph-width": `${Math.max(40, repository.commits.reduce((widest, { lane }) => Math.max(widest, lane), 0) * GRAPH_LANE_WIDTH + 36)}px` } as CSSProperties}>
                      <ol className="commit-list">
                        {repository.commits.map((commit) => (
                          <li className={reachableCommits && !reachableCommits.has(commit.id) ? "commit-dimmed" : undefined} key={commit.id}>
                            <CommitGraph commit={commit} />
                            <code>{commit.id.slice(0, 7)}</code>
                            <span className="commit-refs">
                              {commit.references.map((reference) => (
                                <span className={`ref-chip ref-${reference.kind}`} key={`${reference.kind}-${reference.name}`}>
                                  {reference.name}
                                </span>
                              ))}
                            </span>
                            <span className="commit-subject">{commit.subject}</span>
                            <span className="commit-meta">{commit.author} · {new Date(commit.date).toLocaleString()}</span>
                          </li>
                        ))}
                      </ol>
                    </div>
                  ) : <p className="muted">No commits yet.</p>}
                  {repository.has_more_commits && (
                    <button
                      className="button button-secondary load-more"
                      onClick={() => {
                        loadMoreFailedRef.current = false;
                        void loadMoreCommits();
                      }}
                      disabled={loadingMore}
                    >
                      {loadingMore ? "Loading…" : "Load older commits"}
                    </button>
                  )}
                </>
              )}
            </section>
            <aside className="status-column" aria-label="Repository status">
              <section className="commit-composer" aria-label="Create commit">
                <label htmlFor="commit-message">Commit message</label>
                <textarea
                  id="commit-message"
                  value={commitMessage}
                  onChange={(event) => changeCommitMessage(event.target.value)}
                  onKeyDown={(event) => {
                    if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
                      event.preventDefault();
                      void commitChanges();
                    }
                  }}
                  rows={4}
                  disabled={committing}
                />
                {repository.history_head && (
                  <label className="amend-option">
                    <input
                      type="checkbox"
                      checked={amending}
                      onChange={(event) => {
                        const amend = event.target.checked;
                        setIsAmend(amend);
                        if (amend && !commitMessage && repository.commit.previous_message) {
                          changeCommitMessage(repository.commit.previous_message);
                        }
                      }}
                      disabled={committing || repository.status.merging}
                    />
                    Amend previous commit
                  </label>
                )}
                <button
                  className="button button-primary"
                  type="button"
                  onClick={() => void commitChanges()}
                  disabled={committing || changingStatus || !commitMessage.length || (!amending && !repository.status.staged.length && !repository.status.merging)}
                >
                  {committing ? "Committing…" : amending ? "Amend commit" : repository.status.merging ? "Commit merge" : "Commit staged changes"}
                </button>
                {commitError && <p className="error-message" role="alert">{commitError}</p>}
              </section>
              {repository.status.truncated && (
                <p className="status-truncated" role="status">Change list is truncated at 512 KiB.</p>
              )}
              {statusError && <p className="error-message" role="alert">{statusError}</p>}
              <div className="status-panels">
                {(["staged", "unstaged"] as const).map((group) => {
                  const entries = repository.status[group];
                  const title = group === "staged" ? "Staged changes" : "Unstaged changes";
                  return (
                    <section className="status-pane" aria-label={title} key={group}>
                      <h2>{title} <span className="status-count">{entries.length}</span></h2>
                      {entries.length ? (
                        <ul>
                          {entries.map((entry) => (
                            <li key={`${entry.kind}-${entry.path_bytes.join(",")}`}>
                              <span className={`status-kind status-${entry.kind}`}>{entry.kind}</span>
                              <code title={entry.path}>{entry.path}</code>
                              <button
                                id={`diff-${group}-${entry.path_bytes.join("-")}`}
                                className="status-action"
                                type="button"
                                aria-label={`View diff for ${entry.path}`}
                                onClick={() => void viewFileDiff(entry, group)}
                              >
                                Diff
                              </button>
                              {group === "staged" ? (
                                <button
                                  data-group={group}
                                  className="status-action"
                                  type="button"
                                  aria-label={`Unstage ${entry.path}`}
                                  onClick={() => void changeStatusEntry(entry, "unstage")}
                                  disabled={changingStatus || committing}
                                >
                                  Unstage
                                </button>
                              ) : (
                                <button
                                  data-group={group}
                                  className="status-action"
                                  type="button"
                                  aria-label={`${entry.kind === "conflicting" ? "Stage resolution for" : "Stage"} ${entry.path}`}
                                  onClick={() => void changeStatusEntry(entry, "stage")}
                                  disabled={changingStatus || committing}
                                >
                                  {entry.kind === "conflicting" ? "Stage resolution" : "Stage"}
                                </button>
                              )}
                            </li>
                          ))}
                        </ul>
                      ) : <p className="muted">No changes.</p>}
                    </section>
                  );
                })}
              </div>
            </aside>
          </div>
        </section>
      ) : (
        <section className="welcome-panel">
          <h1>Open a repository</h1>
          <p>Choose a local Git repository to view its branch and recent commits.</p>
          <button className="button button-primary" onClick={() => void openRepository()} disabled={opening} autoFocus={recentRepositories.length === 0}>
            {opening ? "Opening…" : "Open repository"}
          </button>
          {error && <p className="error-message" role="alert">{error}</p>}
          <section className="recent-repositories" aria-label="Recent repositories">
            <label htmlFor="recent-filter">Recent</label>
            <input
              id="recent-filter"
              type="search"
              placeholder="Search recent repositories"
              value={recentFilter}
              onChange={(event) => setRecentFilter(event.target.value)}
              autoFocus={recentRepositories.length > 0}
              onKeyDown={(event) => {
                if (event.key === "ArrowDown" && filteredRecentRepositories.length > 0) {
                  event.preventDefault();
                  recentRepositoryRefs.current[0]?.focus();
                }
              }}
            />
            {/* shortcut: Keep all recent paths rendered; virtualize if measured list sizes slow the welcome screen. */}
            {recentRepositories.length === 0 ? (
              <p className="muted">No recent repositories.</p>
            ) : filteredRecentRepositories.length === 0 ? (
              <p className="muted">No matching repositories.</p>
            ) : (
              <ul>
                {filteredRecentRepositories.map((path, index) => (
                    <li key={path}>
                      <button
                        ref={(element) => { recentRepositoryRefs.current[index] = element; }}
                        className="recent-repository"
                        onClick={() => void openRepository(path)}
                        disabled={opening}
                        onKeyDown={(event) => {
                          if (event.key === "ArrowDown" && index < filteredRecentRepositories.length - 1) {
                            event.preventDefault();
                            recentRepositoryRefs.current[index + 1]?.focus();
                          } else if (event.key === "ArrowUp") {
                            event.preventDefault();
                            if (index === 0) document.getElementById("recent-filter")?.focus();
                            else recentRepositoryRefs.current[index - 1]?.focus();
                          }
                        }}
                      >
                        <strong>{path.split(/[\\/]/).filter(Boolean).slice(-1)[0] ?? path}</strong>
                        <span>{path}</span>
                      </button>
                      <button
                        className="recent-remove"
                        aria-label={`Remove ${path} from recent repositories`}
                        disabled={opening}
                        onClick={() => removeRecentRepository(path, index)}
                      >
                        ×
                      </button>
                    </li>
                ))}
              </ul>
            )}
          </section>
        </section>
      )}
      {identityPrompt && (
        // A modal <dialog>: the page behind is inert and Tab never reaches it. Escape is handled with the other keys.
        <dialog className="identity-dialog" aria-labelledby="identity-title" ref={showModal}>
          <form
            onSubmit={(event) => {
              event.preventDefault();
              if (!identityPrompt.name.trim() || !identityPrompt.email.trim()) {
                setIdentityError("Name and email are required.");
                return;
              }
              void commitChanges(
                { name: identityPrompt.name, email: identityPrompt.email },
                identityPrompt.save,
              );
            }}
          >
            <h2 id="identity-title">Commit author</h2>
            <label htmlFor="author-name">Name</label>
            <input id="author-name" value={identityPrompt.name} onChange={(event) => setIdentityPrompt({ ...identityPrompt, name: event.target.value })} />
            <label htmlFor="author-email">Email</label>
            <input id="author-email" type="email" value={identityPrompt.email} onChange={(event) => setIdentityPrompt({ ...identityPrompt, email: event.target.value })} />
            <label className="amend-option">
              <input type="checkbox" checked={identityPrompt.save} onChange={(event) => setIdentityPrompt({ ...identityPrompt, save: event.target.checked })} />
              Save identity globally
            </label>
            {identityError && <p className="error-message" role="alert">{identityError}</p>}
            <footer>
              <button className="button button-secondary" type="button" disabled={committing} onClick={closeIdentityPrompt}>Cancel</button>
              <button className="button button-primary" type="submit" disabled={committing}>Continue</button>
            </footer>
          </form>
        </dialog>
      )}
    </main>
  );
}

export default App;
