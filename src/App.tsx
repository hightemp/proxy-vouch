import {
  useCallback,
  useDeferredValue,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import ImportFormatHelp from "./ImportFormatHelp";
import {
  Activity,
  ArrowDownToLine,
  ArrowUpDown,
  Check,
  CheckCheck,
  CheckCircle2,
  ChevronDown,
  Clipboard,
  Copy,
  Database,
  FileText,
  FolderOpen,
  Globe2,
  HelpCircle,
  Layers3,
  LockKeyhole,
  MoreHorizontal,
  Play,
  Plus,
  RefreshCw,
  Search,
  Settings2,
  ShieldCheck,
  Square,
  Trash2,
  X,
  XCircle,
  Zap,
} from "lucide-react";
import {
  defaultSettings,
  type AppError,
  type BackupPreview,
  type Preferences,
  type StorageStatus,
  type ExportOptions,
  type ImportOptions,
  type Preview,
  type Row,
  type Settings,
  type Snapshot,
  type Status,
} from "./types";

const emptySnapshot: Snapshot = {
  revision: 0,
  reset: true,
  rows: [],
  running: false,
  runId: 0,
  scheduled: 0,
  completed: 0,
  total: 0,
  counts: {},
};
const initialImport: ImportOptions = {
  format: "auto",
  delimiter: ",",
  header: "auto",
  columns: [],
  sourceName: "Pasted text",
};
const filters = [
  "All",
  "Working",
  "Failed",
  "Inconclusive",
  "Unchecked",
  "Invalid",
  "Cancelled",
] as const;
const rowHeight = 58;

function Modal({
  title,
  subtitle,
  children,
  close,
  wide = false,
}: {
  title: string;
  subtitle?: string;
  children: ReactNode;
  close: () => void;
  wide?: boolean;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = ref.current;
    dialog?.showModal();
    return () => dialog?.close();
  }, []);
  return (
    <dialog
      ref={ref}
      className={`modal ${wide ? "wide" : ""}`}
      onCancel={(e) => {
        e.preventDefault();
        close();
      }}
      aria-label={title}
    >
      <header className="modal-heading">
        <div>
          <h2>{title}</h2>
          {subtitle && <p>{subtitle}</p>}
        </div>
        <button
          className="icon-button"
          aria-label="Close dialog"
          onClick={close}
        >
          <X size={20} />
        </button>
      </header>
      {children}
    </dialog>
  );
}

function StatusBadge({ status }: { status: Status }) {
  return (
    <span className={`status status-${status.toLowerCase()}`}>
      <span />
      {status}
    </span>
  );
}
function ErrorText({ error }: { error: string }) {
  return error ? (
    <div className="inline-error" role="alert">
      <XCircle size={17} />
      <span>{error}</span>
    </div>
  ) : null;
}

export default function App() {
  const [rows, setRows] = useState<Row[]>([]);
  const rowMap = useRef(new Map<number, Row>());
  const revision = useRef(0);
  const [meta, setMeta] = useState(emptySnapshot);
  const metaRef = useRef(meta);
  metaRef.current = meta;
  const [settings, setSettings] = useState<Settings>(defaultSettings);
  const [draft, setDraft] = useState<Settings>(defaultSettings);
  const [theme, setTheme] = useState("system");
  const [draftTheme, setDraftTheme] = useState("system");
  const [preferencesLoaded, setPreferencesLoaded] = useState(false);
  const [storage, setStorage] = useState<StorageStatus | null>(null);
  const [backupScope, setBackupScope] = useState("full");
  const [backupPreview, setBackupPreview] = useState<BackupPreview | null>(
    null,
  );
  const [restoreMode, setRestoreMode] = useState("merge");
  const [restoreSettings, setRestoreSettings] = useState(false);
  const [filter, setFilter] = useState<string>("All");
  const [query, setQuery] = useState("");
  const deferredQuery = useDeferredValue(query);
  const [sort, setSort] = useState("added");
  const [selected, setSelected] = useState(new Set<number>());
  const [removalIds, setRemovalIds] = useState<number[] | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(440);
  const viewport = useRef<HTMLDivElement>(null);
  const [modal, setModal] = useState<
    | "import"
    | "settings"
    | "backup"
    | "export"
    | "details"
    | "edit"
    | "clear"
    | "quit"
    | "help"
    | null
  >(null);
  const [helpReturnTo, setHelpReturnTo] = useState<"import" | null>(null);
  const [detailId, setDetailId] = useState<number | null>(null);
  const detail = rows.find((row) => row.id === detailId);
  const [rawText, setRawText] = useState<string | null>(null);
  const [input, setInput] = useState("");
  const [importOptions, setImportOptions] = useState(initialImport);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [replace, setReplace] = useState(false);
  const [keepDuplicates, setKeepDuplicates] = useState(false);
  const [includeInvalid, setIncludeInvalid] = useState(true);
  const [exportOptions, setExportOptions] = useState<ExportOptions>({
    scope: "Working",
    format: "urls",
    credentials: true,
    ids: [],
  });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [toast, setToast] = useState("");
  const native = isTauri();

  const refresh = useCallback(async () => {
    const snapshot = await invoke<Snapshot>("snapshot", {
      since: revision.current,
    });
    if (snapshot.reset) rowMap.current.clear();
    if (snapshot.rows.length || snapshot.reset) {
      for (const row of snapshot.rows) rowMap.current.set(row.id, row);
      setRows([...rowMap.current.values()]);
      if (snapshot.reset) {
        setSelected((previous) => {
          const remaining = new Set(
            [...previous].filter((id) => rowMap.current.has(id)),
          );
          return remaining.size === previous.size ? previous : remaining;
        });
      }
    }
    revision.current = snapshot.revision;
    setMeta(snapshot);
  }, []);

  useEffect(() => {
    if (!native) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        await refresh();
        const status = await invoke<StorageStatus>("storage_status");
        if (!stopped) setStorage(status);
      } catch {
        if (!stopped)
          setError(
            "The desktop connection was interrupted. Restart the app if it persists.",
          );
      }
      if (!stopped) timer = setTimeout(poll, 300);
    };
    void poll();
    void invoke<Preferences>("load_preferences")
      .then((prefs) => {
        if (stopped) return;
        setTheme(prefs.theme);
        setSettings(prefs.check);
        setPreferencesLoaded(true);
      })
      .catch(() =>
        setError(
          "Could not load saved settings. Restart the app before editing settings.",
        ),
      );
    const unlisten = getCurrentWindow().onCloseRequested((event) => {
      event.preventDefault();
      setError("");
      if (metaRef.current.running) setModal("quit");
      else void quit();
    });
    return () => {
      stopped = true;
      clearTimeout(timer);
      void unlisten.then((fn) => fn());
    };
  }, [native, refresh]);

  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      document.documentElement.dataset.theme =
        theme === "system" ? (media.matches ? "dark" : "light") : theme;
    };
    apply();
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [theme]);
  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(""), 4500);
    return () => clearTimeout(timer);
  }, [toast]);
  useEffect(() => {
    const element = viewport.current;
    if (!element) return;
    const observer = new ResizeObserver((entries) =>
      setViewportHeight(entries[0].contentRect.height),
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [rows.length > 0]);

  const sortedRows = useMemo(() => {
    if (sort === "added") return rows;
    const result = [...rows];
    result.sort((a, b) => {
      if (sort === "latency")
        return (
          (a.result?.latencyMs ?? Infinity) -
            (b.result?.latencyMs ?? Infinity) || a.id - b.id
        );
      if (sort === "checked")
        return (
          (b.result?.checkedAt ?? "").localeCompare(
            a.result?.checkedAt ?? "",
          ) || a.id - b.id
        );
      if (sort === "status")
        return a.status.localeCompare(b.status) || a.id - b.id;
      if (sort === "protocol")
        return a.protocol.localeCompare(b.protocol) || a.id - b.id;
      return (
        a.address.localeCompare(b.address, undefined, { numeric: true }) ||
        a.id - b.id
      );
    });
    return result;
  }, [rows, sort]);
  const filteredRows = useMemo(() => {
    const q = deferredQuery.toLowerCase();
    return sortedRows.filter(
      (row) =>
        (filter === "All" || row.status === filter) &&
        (!q ||
          `${row.address} ${row.username} ${row.label}`
            .toLowerCase()
            .includes(q)),
    );
  }, [sortedRows, filter, deferredQuery]);
  const hasHiddenRemoval = useMemo(() => {
    if (modal !== "clear" || removalIds === null) return false;
    const visibleIds = new Set(filteredRows.map((row) => row.id));
    return removalIds.some((id) => !visibleIds.has(id));
  }, [modal, removalIds, filteredRows]);
  useEffect(() => {
    if (viewport.current) viewport.current.scrollTop = 0;
    setScrollTop(0);
  }, [filter, deferredQuery, sort]);
  const startIndex = Math.max(0, Math.floor(scrollTop / rowHeight) - 5);
  const visibleRows = filteredRows.slice(
    startIndex,
    startIndex + Math.ceil(viewportHeight / rowHeight) + 12,
  );
  const count = (status: Status) => meta.counts[status] ?? 0;
  const tested = count("Working") + count("Failed") + count("Inconclusive");
  const successRate = tested
    ? Math.round((count("Working") / tested) * 100)
    : null;
  const allSelected =
    filteredRows.length > 0 &&
    filteredRows.every((row) => selected.has(row.id));

  async function run<T>(action: () => Promise<T>): Promise<T | undefined> {
    setError("");
    setBusy(true);
    try {
      return await action();
    } catch (err) {
      setError(
        typeof err === "object" && err && "message" in err
          ? (err as AppError).message
          : String(err),
      );
    } finally {
      setBusy(false);
    }
  }
  function openModal(value: typeof modal) {
    setError("");
    setModal(value);
  }
  function openImport() {
    setInput("");
    setPreview(null);
    setImportOptions(initialImport);
    setReplace(false);
    openModal("import");
  }
  async function chooseFile() {
    await run(async () => {
      const file = await invoke<{ text: string; sourceName: string } | null>(
        "import_file",
      );
      if (file) {
        setInput(file.text);
        setImportOptions({ ...initialImport, sourceName: file.sourceName });
        setPreview(null);
        setModal("import");
      }
    });
  }
  async function loadDropped(file: File) {
    if (meta.running) return;
    await run(async () => {
      if (file.size > 20 * 1024 * 1024)
        throw new Error("Import must not exceed 20 MiB.");
      const text = new TextDecoder("utf-8", { fatal: true }).decode(
        await file.arrayBuffer(),
      );
      setInput(text);
      setImportOptions({ ...initialImport, sourceName: file.name });
      setPreview(null);
      setModal("import");
    });
  }
  async function startCheck(ids: number[], detectAgain = false) {
    await run(async () => {
      await invoke("start_check", { ids, settings, detectAgain });
      await refresh();
    });
  }
  function selectionToggle(id: number) {
    setSelected((previous) => {
      const next = new Set(previous);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }
  function toggleAll() {
    setSelected((previous) => {
      const next = new Set(previous);
      for (const row of filteredRows) {
        if (allSelected) next.delete(row.id);
        else next.add(row.id);
      }
      return next;
    });
  }
  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (
        (event.ctrlKey || event.metaKey) &&
        event.key.toLowerCase() === "a" &&
        !modal &&
        !(event.target instanceof HTMLInputElement)
      ) {
        event.preventDefault();
        setSelected(new Set(filteredRows.map((row) => row.id)));
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [filteredRows, modal]);
  function exportIds(scope: string) {
    return scope === "Selected"
      ? sortedRows.filter((row) => selected.has(row.id)).map((row) => row.id)
      : scope === "Filtered"
        ? filteredRows.map((row) => row.id)
        : sortedRows.map((row) => row.id);
  }
  function openExport(scope: string) {
    setExportOptions({
      scope,
      format: scope === "Working" ? "urls" : "original",
      credentials: true,
      ids: exportIds(scope),
    });
    openModal("export");
  }
  async function sendExport(
    destination: "file" | "clipboard",
    options = exportOptions,
  ) {
    return run(async () => {
      const count = await invoke<number | null>("export_data", {
        options: { ...options, ids: exportIds(options.scope) },
        destination,
      });
      if (count !== null) {
        setToast(
          `${count.toLocaleString()} ${count === 1 ? "record" : "records"} ${destination === "file" ? "saved" : "copied to clipboard"}.`,
        );
        setModal(null);
      }
      return count;
    });
  }
  async function quit(withoutSaving = false) {
    await run(async () => {
      if (metaRef.current.running) {
        await invoke("stop_check");
        for (let i = 0; i < 100; i++) {
          const snapshot = await invoke<Snapshot>("snapshot", {
            since: revision.current,
          });
          if (!snapshot.running) break;
          await new Promise((resolve) => setTimeout(resolve, 50));
          if (i === 99)
            throw new Error("Checks are still stopping. Please try again.");
        }
      }
      if (!withoutSaving) {
        try {
          await invoke("flush_workspace");
        } catch (error) {
          setModal("quit");
          throw error;
        }
      }
      await getCurrentWindow().destroy();
    });
  }
  const exportCount =
    exportOptions.scope === "Selected"
      ? selected.size
      : exportOptions.scope === "Filtered"
        ? filteredRows.length
        : exportOptions.scope === "All"
          ? rows.length
          : exportOptions.scope === "Checked"
            ? tested
            : count(exportOptions.scope as Status);

  return (
    <div
      className="app"
      onDragOver={(event) => {
        event.preventDefault();
      }}
      onDrop={(event) => {
        event.preventDefault();
        const file = event.dataTransfer.files[0];
        if (file) void loadDropped(file);
      }}
    >
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-icon">
            <Activity size={25} />
          </span>
          <span>
            Proxy<span className="brand-light">Vouch</span>
          </span>
        </div>
        <div className="nav-caption">WORKSPACE</div>
        <button
          className="nav-item active"
          onClick={() => {
            setFilter("All");
            setQuery("");
          }}
        >
          <Layers3 size={18} />
          Proxy checker
          <span className="nav-count">{rows.length.toLocaleString()}</span>
        </button>
        <button
          className="nav-item"
          disabled={native && !preferencesLoaded}
          onClick={() => {
            setDraft(settings);
            setDraftTheme(theme);
            openModal("settings");
          }}
        >
          <Settings2 size={18} />
          Settings
        </button>
        <button className="nav-item" onClick={() => openModal("backup")}>
          <Database size={18} />
          Backup & restore
        </button>
        <div className="sidebar-bottom">
          <div className="local-note">
            <ShieldCheck size={20} />
            <strong>Local by design</strong>
            <p>
              Your list stays on this device.
              <br />
              No account. No telemetry.
            </p>
          </div>
          <button
            className="nav-item help"
            onClick={() => {
              setHelpReturnTo(null);
              openModal("help");
            }}
          >
            <HelpCircle size={17} />
            Formats & help
          </button>
          <div className="version">
            ProxyVouch <span>v{__APP_VERSION__}</span>
          </div>
        </div>
      </aside>
      <main>
        <header className="page-header">
          <div>
            <div className="eyebrow">YOUR NETWORK, AT A GLANCE</div>
            <h1>
              Proxy checker
              <span className="session-label">
                {!native
                  ? "Desktop preview"
                  : storage?.error
                    ? "Not saved"
                    : storage?.savedRevision != null &&
                        storage.savedRevision >= meta.revision
                      ? "Saved locally"
                      : "Saving…"}
              </span>
            </h1>
            <p>Check your proxies. Keep the ones that work.</p>
          </div>
          <div className="header-actions">
            <button
              disabled={busy || meta.running || !native}
              onClick={() => void chooseFile()}
            >
              <FolderOpen size={17} />
              Import file
            </button>
            <button
              className="primary"
              disabled={busy || meta.running || !native}
              onClick={openImport}
            >
              <Plus size={18} />
              Add proxies
            </button>
          </div>
        </header>
        {!native && (
          <div className="bridge-notice">
            <Globe2 size={18} />
            <span>
              This is a browser preview. Open the Tauri desktop app to import
              and check proxies.
            </span>
          </div>
        )}
        {!modal && <ErrorText error={error} />}
        {storage?.error && (
          <ErrorText
            error={`Changes are not saved. ${storage.error.message}`}
          />
        )}
        {storage?.notice && (
          <p className="storage-notice" role="status">
            {storage.notice}
          </p>
        )}
        <section className="stats" aria-label="Check results">
          <div className="stat-card">
            <span className="stat-icon neutral">
              <Layers3 size={19} />
            </span>
            <span className="stat-label">Total proxies</span>
            <strong>{rows.length.toLocaleString()}</strong>
            <small>
              {meta.running
                ? `${count("Queued")} queued · ${count("Checking")} checking`
                : "In your saved list"}
            </small>
          </div>
          <div className="stat-card">
            <span className="stat-icon green">
              <CheckCircle2 size={19} />
            </span>
            <span className="stat-label">Working</span>
            <strong>{count("Working").toLocaleString()}</strong>
            <small>
              <span className="tiny-dot green-dot" />
              Request verified
            </small>
          </div>
          <div className="stat-card">
            <span className="stat-icon red">
              <XCircle size={19} />
            </span>
            <span className="stat-label">Failed</span>
            <strong>{count("Failed").toLocaleString()}</strong>
            <small>
              <span className="tiny-dot red-dot" />
              Connection or proxy error
            </small>
          </div>
          <div className="stat-card">
            <span className="stat-icon amber">
              <Activity size={19} />
            </span>
            <span className="stat-label">Success rate</span>
            <strong>
              {successRate === null ? (
                <span className="empty-number">—</span>
              ) : (
                <>
                  {successRate}
                  <span className="percent">%</span>
                </>
              )}
            </strong>
            <small>
              {tested
                ? `${tested} checked · ${count("Inconclusive")} inconclusive`
                : "Available after your first check"}
            </small>
          </div>
        </section>
        <section className="list-panel">
          <div className="list-heading">
            <div>
              <h2>
                Proxy list{" "}
                <span className="number-tag">
                  {rows.length.toLocaleString()}
                </span>
              </h2>
              <p>
                {meta.running
                  ? "Checking your list. Results appear as they arrive."
                  : "Import a list to get started, or drop a file anywhere."}
              </p>
            </div>
            <div className="check-actions">
              {!meta.running &&
                selected.size === 0 &&
                (filter === "Failed" || filter === "Inconclusive") &&
                filteredRows.length > 0 && (
                  <button
                    disabled={busy}
                    onClick={() =>
                      void startCheck(filteredRows.map((row) => row.id))
                    }
                  >
                    <RefreshCw size={14} />
                    Retry {filter.toLowerCase()}
                  </button>
                )}
              {selected.size > 0 && (
                <>
                  <button
                    disabled={busy || meta.running}
                    onClick={() => void startCheck([...selected])}
                  >
                    Check selected ({selected.size})
                  </button>
                  <button
                    className="danger"
                    disabled={busy || meta.running}
                    onClick={() => {
                      const ids = rows
                        .filter((row) => selected.has(row.id))
                        .map((row) => row.id);
                      if (!ids.length) return;
                      setRemovalIds(ids);
                      openModal("clear");
                    }}
                  >
                    <Trash2 size={15} />
                    Remove selected ({selected.size})
                  </button>
                </>
              )}
              {meta.running ? (
                <button
                  className="stop"
                  disabled={busy}
                  onClick={() =>
                    void run(async () => {
                      await invoke("stop_check");
                      setToast(
                        "Stopping checks… Completed results will be kept.",
                      );
                    })
                  }
                >
                  <Square size={14} fill="currentColor" />
                  Stop checking
                </button>
              ) : (
                <button
                  className="primary"
                  disabled={busy || !rows.some((r) => r.status !== "Invalid")}
                  onClick={() => void startCheck(rows.map((r) => r.id))}
                >
                  <Play size={15} fill="currentColor" />
                  Check all
                </button>
              )}
              <button
                className="icon-button"
                title="Clear list"
                aria-label="Clear list"
                disabled={busy || meta.running || !rows.length}
                onClick={() => {
                  setRemovalIds(null);
                  openModal("clear");
                }}
              >
                <Trash2 size={17} />
              </button>
            </div>
          </div>
          {meta.scheduled > 0 && (
            <div className="run-progress" aria-label="Check progress">
              <div
                style={{
                  width: `${((meta.running ? meta.completed : meta.scheduled) / meta.scheduled) * 100}%`,
                }}
              />
            </div>
          )}
          <div className="filter-bar">
            <div
              className="filter-tabs"
              role="group"
              aria-label="Filter by status"
            >
              {filters.map((value) => (
                <button
                  key={value}
                  className={filter === value ? "selected" : ""}
                  onClick={() => setFilter(value)}
                >
                  {value}
                  {value === "All"
                    ? null
                    : count(value as Status) > 0 && (
                        <span>{count(value as Status)}</span>
                      )}
                </button>
              ))}
            </div>
            <div className="search-box">
              <Search size={16} />
              <input
                aria-label="Search proxies"
                placeholder="Search proxies…"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
              />
            </div>
          </div>
          <div className="table-head">
            <input
              type="checkbox"
              aria-label="Select all filtered proxies"
              checked={allSelected}
              onChange={toggleAll}
              disabled={!filteredRows.length}
            />
            <button
              onClick={() => setSort(sort === "address" ? "added" : "address")}
            >
              PROXY <ArrowUpDown size={12} />
            </button>
            <button onClick={() => setSort("protocol")}>PROTOCOL</button>
            <button onClick={() => setSort("status")}>STATUS</button>
            <button onClick={() => setSort("latency")}>
              LATENCY <ArrowUpDown size={12} />
            </button>
            <span>EXIT IP</span>
            <button onClick={() => setSort("checked")}>LAST CHECKED</button>
            <span />
          </div>
          {!rows.length ? (
            <div className="empty-state">
              <div className="empty-art">
                <div className="orbit orbit-one" />
                <div className="orbit orbit-two" />
                <div className="empty-art-icon">
                  <Globe2 size={36} strokeWidth={1.3} />
                  <span>
                    <Check size={14} strokeWidth={3} />
                  </span>
                </div>
                <span className="orbit-dot dot-one" />
                <span className="orbit-dot dot-two" />
              </div>
              <h3>A clear view of your proxies starts here</h3>
              <p>
                Paste your list or import a file. We’ll recognize the format
                <br />
                and detect the protocol when it’s missing.
              </p>
              <button
                className="primary"
                disabled={!native || busy}
                onClick={openImport}
              >
                <Plus size={17} />
                Add your first proxies
              </button>
              <div className="format-chips">
                <span>HTTP / HTTPS</span>
                <span>SOCKS4 / SOCKS5</span>
                <span>TXT · CSV · TSV</span>
              </div>
            </div>
          ) : (
            <div
              className="table-viewport"
              ref={viewport}
              onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
            >
              {filteredRows.length === 0 ? (
                <div className="no-results">
                  <Search size={27} />
                  <h3>No matching proxies</h3>
                  <p>Try another search or status filter.</p>
                  <button
                    onClick={() => {
                      setQuery("");
                      setFilter("All");
                    }}
                  >
                    Reset filters
                  </button>
                </div>
              ) : (
                <div
                  style={{
                    height: filteredRows.length * rowHeight,
                    position: "relative",
                  }}
                >
                  {visibleRows.map((row, index) => (
                    <div
                      className={`proxy-row ${selected.has(row.id) ? "row-selected" : ""}`}
                      key={row.id}
                      style={{
                        position: "absolute",
                        top: (startIndex + index) * rowHeight,
                        height: rowHeight,
                        width: "100%",
                      }}
                    >
                      <input
                        type="checkbox"
                        aria-label={`Select ${row.address}`}
                        checked={selected.has(row.id)}
                        onChange={() => selectionToggle(row.id)}
                      />
                      <button
                        className="address-button"
                        onClick={() => {
                          setDetailId(row.id);
                          setRawText(null);
                          openModal("details");
                        }}
                      >
                        <span>
                          {row.address}
                          {row.hasCredentials && <LockKeyhole size={12} />}
                        </span>
                        <small>
                          {row.label ||
                            (row.hasCredentials
                              ? "Credentials supplied"
                              : `${row.source} · line ${row.line}`)}
                        </small>
                      </button>
                      <span
                        className={`protocol-tag ${row.protocol === "auto" ? "auto" : ""}`}
                      >
                        {row.protocol === "auto" ? (
                          <>
                            <Zap size={12} />
                            Auto
                          </>
                        ) : (
                          row.protocol.toUpperCase()
                        )}
                      </span>
                      <StatusBadge status={row.status} />
                      <span className="mono latency">
                        {row.result?.latencyMs != null ? (
                          <>
                            {row.result.latencyMs}
                            <small>ms</small>
                          </>
                        ) : (
                          "—"
                        )}
                      </span>
                      <span className="mono exit-ip">
                        {row.result?.exitIp ?? "—"}
                      </span>
                      <span className="checked-at">
                        {row.result
                          ? new Date(row.result.checkedAt).toLocaleTimeString(
                              [],
                              {
                                hour: "2-digit",
                                minute: "2-digit",
                                second: "2-digit",
                              },
                            )
                          : "Not checked"}
                      </span>
                      <button
                        className="icon-button"
                        aria-label={`Details for ${row.address}`}
                        onClick={() => {
                          setDetailId(row.id);
                          setRawText(null);
                          openModal("details");
                        }}
                      >
                        <MoreHorizontal size={18} />
                      </button>
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}
          <div className="table-footer">
            <span>
              {selected.size > 0
                ? `${selected.size.toLocaleString()} selected · `
                : ""}
              {filteredRows.length.toLocaleString()}{" "}
              {filteredRows.length === 1 ? "proxy" : "proxies"}
              {filter !== "All" || query
                ? ` of ${rows.length.toLocaleString()}`
                : ""}
            </span>
            <label className="sort-select">
              <ArrowUpDown size={14} />
              <select
                aria-label="Sort proxies"
                value={sort}
                onChange={(e) => setSort(e.target.value)}
              >
                <option value="added">Import order</option>
                <option value="address">Address</option>
                <option value="status">Status</option>
                <option value="protocol">Protocol</option>
                <option value="latency">Fastest first</option>
                <option value="checked">Recently checked</option>
              </select>
            </label>
            <span className="check-profile">
              <span
                className={`tiny-dot ${meta.running ? "green-dot pulse" : ""}`}
              />
              {meta.running
                ? `${meta.completed} / ${meta.scheduled} completed`
                : settings.ipEcho
                  ? `${settings.url.toLowerCase().startsWith("https:") ? "HTTPS" : "HTTP"} IP check`
                  : "Custom URL check"}
            </span>
          </div>
        </section>
        <section className="export-strip">
          <div className="export-intro">
            <span>
              <ArrowDownToLine size={20} />
            </span>
            <div>
              <h3>Take your results with you</h3>
              <p>Copy a clean list or save a report.</p>
            </div>
          </div>
          <div className="export-buttons">
            <button
              disabled={!count("Working") || busy}
              onClick={() =>
                void sendExport("clipboard", {
                  scope: "Working",
                  format: "urls",
                  credentials: true,
                  ids: [],
                })
              }
            >
              <Copy size={15} />
              Copy working
            </button>
            <button
              disabled={!count("Working") || busy}
              onClick={() =>
                void sendExport("file", {
                  scope: "Working",
                  format: "urls",
                  credentials: true,
                  ids: [],
                })
              }
            >
              <ArrowDownToLine size={15} />
              Save working
            </button>
            <button
              disabled={!count("Failed") || busy}
              onClick={() =>
                void sendExport("clipboard", {
                  scope: "Failed",
                  format: "original",
                  credentials: true,
                  ids: [],
                })
              }
            >
              <Copy size={15} />
              Copy failed
            </button>
            <button
              disabled={!count("Failed") || busy}
              onClick={() =>
                void sendExport("file", {
                  scope: "Failed",
                  format: "original",
                  credentials: true,
                  ids: [],
                })
              }
            >
              <ArrowDownToLine size={15} />
              Save failed
            </button>
            <button
              disabled={!rows.length || busy}
              onClick={() => openExport(selected.size ? "Selected" : "All")}
            >
              More <ChevronDown size={14} />
            </button>
          </div>
        </section>
        <footer className="page-footer">
          <span>
            <ShieldCheck size={14} />A real request verifies every working
            proxy.
          </span>
          <span>Results reflect the selected endpoint and time of check.</span>
        </footer>
      </main>
      {toast && (
        <div className="toast" role="status">
          <CheckCircle2 size={18} />
          {toast}
          <button
            aria-label="Dismiss notification"
            onClick={() => setToast("")}
          >
            <X size={14} />
          </button>
        </div>
      )}

      {modal === "import" && (
        <Modal
          title="Add proxies"
          subtitle="Paste a mixed list or import TXT, CSV or TSV. Missing protocols are detected automatically."
          close={() => !busy && setModal(null)}
          wide
        >
          <div className="modal-body">
            <ErrorText error={error} />
            <div className="input-toolbar">
              <strong>{importOptions.sourceName}</strong>
              <div>
                <button
                  disabled={busy}
                  onClick={() => {
                    setHelpReturnTo("import");
                    openModal("help");
                  }}
                >
                  <HelpCircle size={15} />
                  Supported formats
                </button>
                <button
                  disabled={busy}
                  onClick={() =>
                    void run(async () => {
                      const text = await invoke<string>("read_clipboard");
                      setInput(text);
                      setPreview(null);
                    })
                  }
                >
                  <Clipboard size={15} />
                  Paste from clipboard
                </button>
                <button disabled={busy} onClick={() => void chooseFile()}>
                  <FolderOpen size={15} />
                  Choose file
                </button>
              </div>
            </div>
            <textarea
              className="proxy-input"
              aria-label="Proxy list input"
              spellCheck={false}
              placeholder={
                "192.0.2.10:8080\nproxy.example:1080:demo-user:demo-pass socks5\nhttps://demo-user:demo-pass@proxy.example:8443"
              }
              value={input}
              onChange={(e) => {
                setInput(e.target.value);
                setPreview(null);
              }}
            />
            <div className="form-grid import-grid">
              <label>
                Input format
                <select
                  value={importOptions.format}
                  onChange={(e) => {
                    setImportOptions({
                      ...importOptions,
                      format: e.target.value,
                    });
                    setPreview(null);
                  }}
                >
                  <option value="auto">Detect format</option>
                  <option value="text">Text / proxy URLs</option>
                  <option value="csv">CSV</option>
                  <option value="tsv">TSV</option>
                  <option value="reverse">username:password:host:port</option>
                </select>
              </label>
              <label>
                Delimiter
                <select
                  value={importOptions.delimiter}
                  onChange={(e) => {
                    setImportOptions({
                      ...importOptions,
                      delimiter: e.target.value,
                    });
                    setPreview(null);
                  }}
                >
                  <option value=",">Comma (,)</option>
                  <option value=";">Semicolon (;)</option>
                </select>
              </label>
              <label>
                Header row
                <select
                  value={importOptions.header}
                  onChange={(e) => {
                    setImportOptions({
                      ...importOptions,
                      header: e.target.value,
                    });
                    setPreview(null);
                  }}
                >
                  <option value="auto">Detect header</option>
                  <option value="yes">First row is a header</option>
                  <option value="no">No header</option>
                </select>
              </label>
            </div>
            <label className="mapping-label">
              Column mapping{" "}
              <span>
                Optional, comma-separated field names. Use “ignore” to skip a
                column.
              </span>
              <input
                aria-label="Column mapping"
                placeholder="host,port,username,password,protocol"
                value={importOptions.columns.join(",")}
                onChange={(e) => {
                  setImportOptions({
                    ...importOptions,
                    columns: e.target.value ? e.target.value.split(",") : [],
                  });
                  setPreview(null);
                }}
              />
            </label>
            {preview && (
              <div className="preview">
                <div className="preview-summary">
                  <span>
                    <CheckCircle2 size={15} />
                    {preview.valid} valid
                  </span>
                  <span>{preview.invalid} invalid</span>
                  <span>{preview.duplicates} duplicates</span>
                  <span>{preview.ignored} ignored</span>
                </div>
                <div className="preview-rows">
                  {preview.rows.map((row) => (
                    <div key={row.id}>
                      <small>{row.line}</small>
                      <code>{row.address}</code>
                      <span>{row.requestedProtocol}</span>
                      {row.hasCredentials && <LockKeyhole size={13} />}
                      <span className={row.error ? "error-text" : "subtle"}>
                        {row.error?.message ??
                          (row.hasCredentials
                            ? "Credentials hidden"
                            : "Ready to import")}
                      </span>
                    </div>
                  ))}
                </div>
                {preview.total > 200 && (
                  <p className="hint">
                    Showing the first 200 of {preview.total.toLocaleString()}{" "}
                    records.
                  </p>
                )}
              </div>
            )}
            <div className="checkbox-line">
              <label>
                <input
                  type="checkbox"
                  checked={replace}
                  onChange={(e) => setReplace(e.target.checked)}
                />
                Replace current list
              </label>
              <label>
                <input
                  type="checkbox"
                  checked={keepDuplicates}
                  onChange={(e) => setKeepDuplicates(e.target.checked)}
                />
                Keep duplicates
              </label>
              <label>
                <input
                  type="checkbox"
                  checked={includeInvalid}
                  onChange={(e) => setIncludeInvalid(e.target.checked)}
                />
                Keep invalid rows for editing
              </label>
            </div>
          </div>
          <footer className="modal-footer">
            <span className="hint">
              UTF-8 · Up to 100,000 records · Saved locally with credentials
            </span>
            <button
              disabled={busy || !input.trim()}
              onClick={() =>
                void run(async () => {
                  const result = await invoke<Preview>("preview_import", {
                    text: input,
                    options: importOptions,
                  });
                  setPreview(result);
                })
              }
            >
              {busy ? "Reading…" : "Preview import"}
            </button>
            <button
              className="primary"
              disabled={busy || !preview || (!preview.valid && !includeInvalid)}
              onClick={() =>
                void run(async () => {
                  const added = await invoke<number>("commit_import", {
                    replace,
                    keepDuplicates,
                    includeInvalid,
                  });
                  setInput("");
                  setPreview(null);
                  setModal(null);
                  setSelected(new Set());
                  await refresh();
                  setToast(`${added.toLocaleString()} records added.`);
                })
              }
            >
              <Plus size={16} />
              Import{" "}
              {preview
                ? (includeInvalid
                    ? preview.total
                    : preview.valid
                  ).toLocaleString()
                : ""}
            </button>
          </footer>
        </Modal>
      )}

      {modal === "settings" && (
        <Modal
          title="Check settings"
          subtitle="Changes apply to the next run. Existing results keep their original profile."
          close={() => !busy && setModal(null)}
        >
          <div className="modal-body">
            <ErrorText error={error} />
            <label className="full-label">
              Check URL
              <input
                value={draft.url}
                onChange={(e) => setDraft({ ...draft, url: e.target.value })}
              />
            </label>
            <label className="full-label">
              Fallback URL <span>Optional; uses the same response rules</span>
              <input
                placeholder="https://your-check-endpoint.example/ip"
                value={draft.fallbackUrl}
                onChange={(e) =>
                  setDraft({ ...draft, fallbackUrl: e.target.value })
                }
              />
            </label>
            <label className="checkbox-label">
              <input
                type="checkbox"
                checked={draft.ipEcho}
                onChange={(e) =>
                  setDraft({ ...draft, ipEcho: e.target.checked })
                }
              />
              Require JSON with a valid “ip” field
            </label>
            <div className="form-grid">
              <label>
                Expected HTTP status
                <input
                  type="number"
                  min={100}
                  max={599}
                  value={draft.expectedStatus}
                  onChange={(e) =>
                    setDraft({
                      ...draft,
                      expectedStatus: Number(e.target.value),
                    })
                  }
                />
              </label>
              <label>
                Response must contain
                <input
                  placeholder="Optional text"
                  value={draft.bodyContains}
                  onChange={(e) =>
                    setDraft({ ...draft, bodyContains: e.target.value })
                  }
                />
              </label>
              <label>
                Concurrent checks
                <input
                  type="number"
                  min={1}
                  max={200}
                  value={draft.concurrency}
                  onChange={(e) =>
                    setDraft({ ...draft, concurrency: Number(e.target.value) })
                  }
                />
              </label>
              <label>
                New attempts per second
                <input
                  type="number"
                  min={1}
                  max={100}
                  value={draft.rateLimit}
                  onChange={(e) =>
                    setDraft({ ...draft, rateLimit: Number(e.target.value) })
                  }
                />
              </label>
              <label>
                Connect timeout (seconds)
                <input
                  type="number"
                  min={1}
                  max={30}
                  value={draft.connectTimeoutMs / 1000}
                  onChange={(e) =>
                    setDraft({
                      ...draft,
                      connectTimeoutMs: Number(e.target.value) * 1000,
                    })
                  }
                />
              </label>
              <label>
                Attempt timeout (seconds)
                <input
                  type="number"
                  min={2}
                  max={60}
                  value={draft.attemptTimeoutMs / 1000}
                  onChange={(e) =>
                    setDraft({
                      ...draft,
                      attemptTimeoutMs: Number(e.target.value) * 1000,
                    })
                  }
                />
              </label>
              <label>
                Total per proxy (seconds)
                <input
                  type="number"
                  min={5}
                  max={300}
                  value={draft.totalTimeoutMs / 1000}
                  onChange={(e) =>
                    setDraft({
                      ...draft,
                      totalTimeoutMs: Number(e.target.value) * 1000,
                    })
                  }
                />
              </label>
              <label>
                Temporary error retries
                <input
                  type="number"
                  min={0}
                  max={2}
                  value={draft.retries}
                  onChange={(e) =>
                    setDraft({ ...draft, retries: Number(e.target.value) })
                  }
                />
              </label>
            </div>
            <p className="hint">
              Auto detection may need five attempts. A short total timeout can
              leave the result inconclusive. All settings, including check URLs,
              are saved on this device.
            </p>
            <label className="full-label">
              Appearance
              <select
                value={draftTheme}
                onChange={(e) => setDraftTheme(e.target.value)}
              >
                <option value="system">System theme</option>
                <option value="light">Light</option>
                <option value="dark">Dark</option>
              </select>
            </label>
          </div>
          <footer className="modal-footer">
            <button
              onClick={() => {
                setDraft(defaultSettings);
                setDraftTheme("system");
              }}
            >
              Restore defaults
            </button>
            <button
              className="primary"
              disabled={busy || !native || !preferencesLoaded}
              onClick={() =>
                void run(async () => {
                  if (
                    draft.connectTimeoutMs > draft.attemptTimeoutMs ||
                    draft.attemptTimeoutMs > draft.totalTimeoutMs
                  )
                    throw new Error(
                      "Connect timeout must be no greater than attempt timeout, and attempt timeout no greater than total timeout.",
                    );
                  await invoke("save_preferences", {
                    preferences: {
                      theme: draftTheme,
                      check: draft,
                    },
                  });
                  setSettings(draft);
                  setTheme(draftTheme);
                  setModal(null);
                  setToast("Settings saved for the next run.");
                })
              }
            >
              Save settings
            </button>
          </footer>
        </Modal>
      )}

      {modal === "backup" && (
        <Modal
          title="Backup & restore"
          subtitle="Move your proxies and settings to another installation, or keep a backup."
          close={() => !busy && setModal(null)}
        >
          <div className="modal-body backup-body">
            <ErrorText error={error} />
            <section className="storage-summary">
              <strong>Automatic saving</strong>
              <p>
                Your proxy list, credentials, last results and settings are
                restored when you reopen the app.
              </p>
              {storage && (
                <code className="storage-path">{storage.directory}</code>
              )}
              <p className="hint">
                Local files and backups include passwords and saved check URLs
                without encryption.
              </p>
              {storage?.error && (
                <button
                  disabled={busy}
                  onClick={() =>
                    void run(async () => {
                      await invoke("flush_workspace");
                      setStorage(await invoke<StorageStatus>("storage_status"));
                      setToast("Workspace saved.");
                    })
                  }
                >
                  Retry saving
                </button>
              )}
            </section>
            <h3 className="subheading">Export a backup</h3>
            <label className="full-label">
              Include in backup
              <select
                value={backupScope}
                onChange={(e) => setBackupScope(e.target.value)}
                disabled={busy}
              >
                <option value="full">
                  Full workspace — proxies, results and settings
                </option>
                <option value="proxies">Proxies and results only</option>
                <option value="settings">Settings only</option>
              </select>
            </label>
            <button
              disabled={busy || !native}
              onClick={() =>
                void run(async () => {
                  const saved = await invoke<boolean>("export_backup", {
                    scope: backupScope,
                  });
                  if (saved) setToast("Backup exported.");
                })
              }
            >
              <ArrowDownToLine size={16} />
              Export backup
            </button>
            <h3 className="subheading">Import a backup</h3>
            <p className="hint">
              Choose a ProxyVouch backup JSON file. For TXT/CSV/TSV lists, use
              Add proxies. Exported reports are not backups.
            </p>
            <button
              disabled={busy || !native || meta.running}
              onClick={() =>
                void run(async () => {
                  setBackupPreview(null);
                  const preview = await invoke<BackupPreview | null>(
                    "preview_backup",
                  );
                  if (preview) {
                    setBackupPreview(preview);
                    setRestoreMode(
                      preview.summary.proxies === null ? "skip" : "merge",
                    );
                    setRestoreSettings(preview.summary.hasSettings);
                  }
                })
              }
            >
              <FolderOpen size={16} />
              Choose backup file
            </button>
            {meta.running && (
              <p className="hint">Stop checking before importing a backup.</p>
            )}
            {backupPreview && (
              <section className="backup-preview" aria-label="Backup preview">
                <strong>{backupPreview.sourceName}</strong>
                <p>
                  {backupPreview.summary.proxies === null
                    ? "No proxy list"
                    : `${backupPreview.summary.proxies.toLocaleString()} records · ${backupPreview.summary.results.toLocaleString()} results · ${backupPreview.summary.invalid.toLocaleString()} invalid`}
                </p>
                <p>
                  {backupPreview.summary.hasSettings
                    ? "Settings included"
                    : "No settings"}
                  {backupPreview.summary.hasCredentials
                    ? " · Contains credentials"
                    : ""}
                </p>
                {backupPreview.summary.proxies !== null && (
                  <label className="full-label">
                    Proxy list
                    <select
                      value={restoreMode}
                      onChange={(e) => setRestoreMode(e.target.value)}
                      disabled={busy}
                    >
                      <option value="merge">
                        Merge — skip duplicates, keep existing results
                      </option>
                      <option value="replace">Replace the current list</option>
                      <option value="skip">Do not import proxies</option>
                    </select>
                  </label>
                )}
                {backupPreview.summary.hasSettings && (
                  <label className="check-label">
                    <input
                      type="checkbox"
                      checked={restoreSettings}
                      disabled={busy}
                      onChange={(e) => setRestoreSettings(e.target.checked)}
                    />
                    Import settings and appearance
                  </label>
                )}
                {restoreMode === "replace" && (
                  <p className="hint">
                    Your {rows.length.toLocaleString()} current records will be
                    replaced with the backup list.
                  </p>
                )}
              </section>
            )}
          </div>
          <footer className="modal-footer">
            <button disabled={busy} onClick={() => setModal(null)}>
              Close
            </button>
            <button
              className="primary"
              disabled={
                busy ||
                !native ||
                meta.running ||
                !backupPreview ||
                (restoreMode === "skip" && !restoreSettings)
              }
              onClick={() =>
                void run(async () => {
                  const result = await invoke<{
                    added: number;
                    skipped: number;
                  }>("restore_backup", {
                    mode: restoreMode,
                    settings: restoreSettings,
                  });
                  const preferences =
                    await invoke<Preferences>("load_preferences");
                  setSettings(preferences.check);
                  setTheme(preferences.theme);
                  setSelected(new Set());
                  await refresh();
                  setBackupPreview(null);
                  setToast(
                    `Backup imported. ${result.added.toLocaleString()} records added, ${result.skipped.toLocaleString()} duplicates skipped.`,
                  );
                })
              }
            >
              {restoreMode === "replace"
                ? "Replace list and import"
                : "Import backup"}
            </button>
          </footer>
        </Modal>
      )}

      {modal === "export" && (
        <Modal
          title="Export proxies"
          subtitle="Choose exactly which records to copy or save."
          close={() => !busy && setModal(null)}
        >
          <div className="modal-body">
            <ErrorText error={error} />
            <div className="export-preview-count">
              <FileText size={26} />
              <div>
                <strong>{exportCount.toLocaleString()} records</strong>
                <span>Snapshot taken when you copy or save</span>
              </div>
            </div>
            <div className="form-grid">
              <label>
                Records
                <select
                  value={exportOptions.scope}
                  onChange={(e) =>
                    setExportOptions({
                      ...exportOptions,
                      scope: e.target.value,
                    })
                  }
                >
                  {[
                    "Working",
                    "Failed",
                    "Checked",
                    "Inconclusive",
                    "Selected",
                    "Filtered",
                    "All",
                  ].map((scope) => (
                    <option key={scope}>{scope}</option>
                  ))}
                </select>
              </label>
              <label>
                Format
                <select
                  value={exportOptions.format}
                  onChange={(e) =>
                    setExportOptions({
                      ...exportOptions,
                      format: e.target.value,
                      credentials: !["csv", "json"].includes(e.target.value),
                    })
                  }
                >
                  <option value="urls">TXT · Proxy URLs</option>
                  <option value="original">Original lines</option>
                  <option value="compact">
                    TXT · host:port:user:pass protocol
                  </option>
                  <option value="csv">CSV · Report</option>
                  <option value="json">JSON · Report</option>
                </select>
              </label>
            </div>
            <label className="checkbox-label">
              <input
                type="checkbox"
                checked={exportOptions.credentials}
                onChange={(e) =>
                  setExportOptions({
                    ...exportOptions,
                    credentials: e.target.checked,
                  })
                }
              />
              Include credentials
            </label>
            <p className="hint">
              {exportOptions.credentials
                ? "Passwords will be included in the clipboard or file."
                : "Credentials will be omitted, not replaced by placeholder passwords."}{" "}
              {exportOptions.format === "csv" &&
                "CSV reports protect against spreadsheet formulas. Use URLs or JSON for exact data transfer."}{" "}
              {exportOptions.format === "urls" &&
                "Records without a verified or explicitly selected protocol need Original lines or a report."}
            </p>
          </div>
          <footer className="modal-footer">
            <button
              disabled={busy || !exportCount}
              onClick={() => void sendExport("clipboard")}
            >
              <Copy size={16} />
              Copy to clipboard
            </button>
            <button
              className="primary"
              disabled={busy || !exportCount}
              onClick={() => void sendExport("file")}
            >
              <ArrowDownToLine size={16} />
              Save file
            </button>
          </footer>
        </Modal>
      )}

      {modal === "details" && detail && (
        <Modal
          title={detail.address}
          subtitle={`${detail.source} · line ${detail.line}`}
          close={() => setModal(null)}
        >
          <div className="modal-body">
            <ErrorText error={error} />
            <div className="detail-status">
              <StatusBadge status={detail.status} />
              <span className="protocol-tag">
                {detail.protocol.toUpperCase()}
              </span>
            </div>
            <dl className="detail-grid">
              <div>
                <dt>Requested protocol</dt>
                <dd>{detail.requestedProtocol.toUpperCase()}</dd>
              </div>
              <div>
                <dt>Detected protocol</dt>
                <dd>
                  {detail.result?.detected?.toUpperCase() ?? "Not confirmed"}
                </dd>
              </div>
              <div>
                <dt>Authentication</dt>
                <dd>
                  {detail.result?.authentication ??
                    (detail.hasCredentials
                      ? "Credentials supplied"
                      : "Not checked")}
                </dd>
              </div>
              <div>
                <dt>Exit IP</dt>
                <dd>{detail.result?.exitIp ?? "Not measured"}</dd>
              </div>
              <div>
                <dt>Request latency</dt>
                <dd>
                  {detail.result?.latencyMs != null
                    ? `${detail.result.latencyMs} ms`
                    : "Not measured"}
                </dd>
              </div>
              <div>
                <dt>Total duration</dt>
                <dd>
                  {detail.result
                    ? `${detail.result.totalDurationMs} ms`
                    : "Not measured"}
                </dd>
              </div>
            </dl>
            {detail.error && <ErrorText error={detail.error.message} />}
            {detail.result && (
              <>
                <div className="result-message">
                  <strong>{detail.result.code || "Request verified"}</strong>
                  <p>{detail.result.message}</p>
                  <code>{detail.result.checkUrl}</code>
                </div>
                <h3 className="subheading">Check attempts</h3>
                <div className="attempts">
                  {detail.result.attempts.map((attempt, index) => (
                    <div key={index}>
                      <span
                        className={`attempt-dot ${attempt.status === "Working" ? "ok" : ""}`}
                      />
                      <div>
                        <strong>
                          {attempt.protocol.toUpperCase()}{" "}
                          <small>
                            {attempt.durationMs} ms · {attempt.stage}
                          </small>
                        </strong>
                        <p>{attempt.message}</p>
                      </div>
                    </div>
                  ))}
                </div>
              </>
            )}
            <button
              className="text-button"
              disabled={busy}
              onClick={() =>
                void run(async () =>
                  setRawText(
                    await invoke<string>("reveal_entry", { id: detail.id }),
                  ),
                )
              }
            >
              <LockKeyhole size={14} />
              Reveal original record
            </button>
            {rawText !== null && <pre className="revealed">{rawText}</pre>}
          </div>
          <footer className="modal-footer">
            <button
              disabled={busy || meta.running}
              onClick={() =>
                void run(async () => {
                  setRawText(
                    await invoke<string>("reveal_entry", { id: detail.id }),
                  );
                  setModal("edit");
                })
              }
            >
              Edit
            </button>
            <button
              disabled={busy || meta.running || detail.status === "Invalid"}
              onClick={() => {
                setModal(null);
                void startCheck([detail.id], true);
              }}
            >
              <RefreshCw size={15} />
              Detect again
            </button>
            <button
              className="primary"
              disabled={busy || meta.running || detail.status === "Invalid"}
              onClick={() => {
                setModal(null);
                void startCheck([detail.id]);
              }}
            >
              <Play size={14} />
              Check proxy
            </button>
          </footer>
        </Modal>
      )}

      {modal === "edit" && detail && (
        <Modal
          title="Edit proxy"
          subtitle="Editing clears the previous result. Use a text record or an encoded proxy URL."
          close={() => setModal(null)}
        >
          <div className="modal-body">
            <ErrorText error={error} />
            <textarea
              className="edit-input"
              aria-label="Edit proxy record"
              spellCheck={false}
              value={rawText ?? ""}
              onChange={(e) => setRawText(e.target.value)}
            />
            <p className="hint">
              Credentials are visible while editing this record.
            </p>
          </div>
          <footer className="modal-footer">
            <button
              className="danger"
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  await invoke("clear_entries", { ids: [detail.id] });
                  setSelected((s) => {
                    const next = new Set(s);
                    next.delete(detail.id);
                    return next;
                  });
                  setModal(null);
                  await refresh();
                })
              }
            >
              Remove proxy
            </button>
            <button
              className="primary"
              disabled={busy || !rawText}
              onClick={() =>
                void run(async () => {
                  await invoke("edit_entry", { id: detail.id, text: rawText });
                  setRawText(null);
                  setModal(null);
                  await refresh();
                })
              }
            >
              Save changes
            </button>
          </footer>
        </Modal>
      )}

      {modal === "clear" && (
        <Modal
          title={
            removalIds === null
              ? "Clear your proxy list?"
              : "Remove selected proxies?"
          }
          subtitle={
            removalIds === null
              ? "This removes records and results from your saved list. Export a backup first if you want to keep them."
              : "Only the selected records and their results will be removed from your saved list."
          }
          close={() => !busy && setModal(null)}
        >
          <div className="modal-body">
            <ErrorText error={error} />
            <p>
              {(removalIds?.length ?? rows.length).toLocaleString()} records
              will be removed.
            </p>
            {hasHiddenRemoval && (
              <p className="hint">
                Your selection includes records hidden by the current filter.
              </p>
            )}
          </div>
          <footer className="modal-footer">
            <button disabled={busy} onClick={() => setModal(null)}>
              Cancel
            </button>
            <button
              className="danger"
              disabled={busy || meta.running || removalIds?.length === 0}
              onClick={() =>
                void run(async () => {
                  // An empty ID list means clear all in the backend. Only the
                  // explicit Clear list action may send that value.
                  if (removalIds !== null && !removalIds.length) return;
                  await invoke("clear_entries", { ids: removalIds ?? [] });
                  setSelected(new Set());
                  setModal(null);
                  setToast(
                    removalIds === null
                      ? "Proxy list cleared."
                      : `${removalIds.length.toLocaleString()} selected records removed.`,
                  );
                  await refresh();
                })
              }
            >
              {removalIds === null ? "Clear list" : "Remove selected"}
            </button>
          </footer>
        </Modal>
      )}

      {modal === "quit" && (
        <Modal
          title={
            meta.running ? "Stop checking and quit?" : "Save before quitting"
          }
          subtitle="Your list, completed results and settings are saved automatically on this device."
          close={() => !busy && setModal(null)}
        >
          <div className="modal-body">
            <ErrorText error={error} />
            <p>
              {meta.running
                ? "Unfinished checks will be cancelled. Completed results will be restored next time."
                : "Saving did not finish. Retry saving, or quit with only the last successfully saved data."}
            </p>
          </div>
          <footer className="modal-footer">
            <button disabled={busy} onClick={() => setModal(null)}>
              {meta.running ? "Keep checking" : "Keep open"}
            </button>
            {!!error && (
              <button disabled={busy} onClick={() => void quit(true)}>
                Quit without saving
              </button>
            )}
            <button
              className="primary"
              disabled={busy}
              onClick={() => void quit()}
            >
              {meta.running ? "Stop, save and quit" : "Retry save and quit"}
            </button>
          </footer>
        </Modal>
      )}

      {modal === "help" && (
        <Modal
          title={
            helpReturnTo === "import"
              ? "Supported import formats"
              : "Formats & help"
          }
          subtitle="Bring your existing lists. ProxyVouch recognizes common proxy formats."
          close={() => setModal(helpReturnTo)}
          wide
        >
          <div className="modal-body">
            <ImportFormatHelp />
            <p>
              Without a protocol, Auto tries HTTPS, SOCKS5 with remote DNS,
              HTTP, SOCKS4a and SOCKS4. Explicit protocols are respected. Use{" "}
              <strong>Detect again</strong> to change a record to Auto.
            </p>
            <p>
              <strong>HTTPS</strong> means TLS to the proxy itself. HTTP proxies
              can also reach HTTPS sites through CONNECT.{" "}
              <strong>socks5h</strong> resolves the destination name on the
              proxy.
            </p>
            <p>
              For CSV/TSV, use a header such as{" "}
              <code>host,port,username,password,protocol</code>, or select a
              column mapping. For <code>username:password:host:port</code>,
              select that input format explicitly.
            </p>
            <h3 className="subheading">Understand your results</h3>
            <p>
              <strong>Working</strong> means a request passed the current
              profile. <strong>Failed</strong> means a connection or proxy
              error. <strong>Inconclusive</strong> includes endpoint failures
              and unsupported authentication methods. Inconclusive records are
              included in Checked, and excluded from Failed.
            </p>
            <p>
              Clipboard and file exports include passwords when{" "}
              <strong>Include credentials</strong> is selected. Lists,
              credentials, results and settings are saved locally between
              sessions. Use
              <strong> Backup & restore</strong> to move them to another
              installation. Stored files and backups are not encrypted. There is
              no cloud upload.
            </p>
          </div>
          <footer className="modal-footer">
            <button className="primary" onClick={() => setModal(helpReturnTo)}>
              <CheckCheck size={17} />
              {helpReturnTo === "import" ? "Back to import" : "Got it"}
            </button>
          </footer>
        </Modal>
      )}
    </div>
  );
}
