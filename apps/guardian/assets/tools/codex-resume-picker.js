#!/usr/bin/env node

const fs = require('fs');
const os = require('os');
const path = require('path');
const readline = require('readline');
const { spawnSync } = require('child_process');

const GUARDIAN_RESUME_PICKER_VERSION = 'GuardianCodexResumePicker/2026-05-19-max-visible-v5';
const DEFAULT_LIMIT = 50;
const DEFAULT_SCAN_LIMIT = 3000;
const PREVIEW_CHARS = 300;
const PREVIEW_CHUNK_SIZE = 60;
const JSONL_PREFIX_BYTES = 2 * 1024 * 1024;

function normalizeText(value) {
  return String(value || '').replace(/\s+/g, ' ').trim();
}

function normalizeCwd(value) {
  if (!value) return '';
  return String(value)
    .replace(/^\\\\\?\\/, '')
    .replace(/^\\\?\\/, '')
    .replace(/\\\\/g, '\\')
    .replace(/\//g, '\\')
    .replace(/\\+$/g, '');
}

function truncate(value, maxChars) {
  const text = normalizeText(value);
  return text.length <= maxChars ? text : `${text.slice(0, Math.max(0, maxChars - 3))}...`;
}

function positiveInt(value, fallback) {
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function usage() {
  console.log(`Usage: node codex-resume-picker.js [options] [query]

Options:
  -q, --query <text>       Filter by title, cwd, id, provider, source, file state, or cli version
  -C, --cwd <path>         Filter by normalized cwd
  -n, --limit <n>          Number of rows to display (default: ${DEFAULT_LIMIT})
      --scan-limit <n>     Number of SQLite rows to inspect (default: ${DEFAULT_SCAN_LIMIT})
      --active-only        Hide archived DB rows
      --include-exec       Include source=exec rows
      --all-sources        Include all source values
      --max-visible        Include cross-cwd, archived, exec, and subagent rows
      --pick               Prompt for a row and run codex resume <id>
      --resume <n|id>      Resume by displayed index or exact session id
      --dry-run            Print actions without running codex resume or restoring archived files
      --no-restore         Do not copy archived JSONL back before resume
      --json               Print selected rows as JSON
      --doctor             Print local Codex session storage diagnostics
      --native-doctor      Print native in-app /resume equivalent visibility diagnostics
  -h, --help               Show this help`);
}

function parseArgs(argv) {
  const options = {
    query: '',
    cwd: '',
    limit: DEFAULT_LIMIT,
    scanLimit: DEFAULT_SCAN_LIMIT,
    activeOnly: false,
    includeExec: false,
    allSources: false,
    maxVisible: false,
    pick: false,
    resumeTarget: '',
    dryRun: false,
    noRestore: false,
    json: false,
    doctor: false,
    nativeDoctor: false,
  };
  const queryParts = [];

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    switch (arg) {
      case '-h':
      case '--help':
        usage();
        process.exit(0);
        break;
      case '-q':
      case '--query':
        options.query = argv[i + 1] || '';
        i += 1;
        break;
      case '-C':
      case '--cwd':
        options.cwd = argv[i + 1] || '';
        i += 1;
        break;
      case '-n':
      case '--limit':
        options.limit = positiveInt(argv[i + 1], options.limit);
        i += 1;
        break;
      case '--scan-limit':
        options.scanLimit = positiveInt(argv[i + 1], options.scanLimit);
        i += 1;
        break;
      case '--active-only':
        options.activeOnly = true;
        break;
      case '--include-exec':
        options.includeExec = true;
        break;
      case '--all-sources':
        options.allSources = true;
        break;
      case '--max-visible':
        options.maxVisible = true;
        break;
      case '--pick':
        options.pick = true;
        break;
      case '--resume':
        options.resumeTarget = argv[i + 1] || '';
        i += 1;
        break;
      case '--dry-run':
        options.dryRun = true;
        break;
      case '--no-restore':
        options.noRestore = true;
        break;
      case '--json':
        options.json = true;
        break;
      case '--doctor':
        options.doctor = true;
        break;
      case '--native-doctor':
        options.nativeDoctor = true;
        break;
      default:
        queryParts.push(arg);
        break;
    }
  }

  if (!options.query && queryParts.length > 0) {
    options.query = queryParts.join(' ');
  }

  options.query = normalizeText(options.query).toLowerCase();
  options.cwd = normalizeCwd(options.cwd);
  if (options.maxVisible) {
    options.activeOnly = false;
    options.includeExec = true;
    options.allSources = true;
  }
  return options;
}

function getCodexHome() {
  return process.env.CODEX_HOME || path.join(os.homedir(), '.codex');
}

function sqliteAvailable() {
  const result = spawnSync(process.env.SQLITE3 || 'sqlite3', ['-version'], {
    encoding: 'utf8',
    windowsHide: true,
  });
  return result.status === 0;
}

function sqliteLiteral(value) {
  return `'${String(value).replace(/'/g, "''")}'`;
}

function runSqliteJson(dbPath, sql) {
  const result = spawnSync(process.env.SQLITE3 || 'sqlite3', ['-json', dbPath, sql], {
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    windowsHide: true,
  });
  if (result.status !== 0) {
    const reason = (result.stderr || result.stdout || '').trim() || `exit ${result.status}`;
    throw new Error(`sqlite3 failed: ${reason}`);
  }
  const output = (result.stdout || '').trim();
  return output ? JSON.parse(output) : [];
}

function latestStateDb(codexHome) {
  let candidates = [];
  try {
    candidates = fs.readdirSync(codexHome, { withFileTypes: true })
      .filter((entry) => entry.isFile() && /^state_\d+\.sqlite$/.test(entry.name))
      .map((entry) => {
        const fullPath = path.join(codexHome, entry.name);
        return {
          path: fullPath,
          index: Number(entry.name.match(/^state_(\d+)\.sqlite$/)[1]),
          mtimeMs: fs.statSync(fullPath).mtimeMs,
        };
      });
  } catch (_) {
    return '';
  }
  candidates.sort((a, b) => (b.index - a.index) || (b.mtimeMs - a.mtimeMs));
  return candidates[0] ? candidates[0].path : '';
}

function sourceAllowed(row, options) {
  if (options.allSources) return true;
  if (row.source === 'cli' || row.thread_source === 'user') return true;
  return options.includeExec && row.source === 'exec';
}

function summarizeSource(value) {
  const source = String(value || '?');
  if (!source.trim().startsWith('{')) return source || '?';
  try {
    const parsed = JSON.parse(source);
    const subagent = parsed && parsed.subagent;
    if (typeof subagent === 'string') return `subagent:${subagent}`;
    if (subagent && typeof subagent === 'object') {
      if (subagent.other) return `subagent:${subagent.other}`;
      const spawn = subagent.thread_spawn;
      if (spawn && typeof spawn === 'object') {
        const role = spawn.agent_role || 'unknown';
        const nickname = spawn.agent_nickname || '';
        return nickname ? `subagent:${role}/${nickname}` : `subagent:${role}`;
      }
    }
  } catch (_) {}
  return truncate(source, 80);
}

function formatSource(source, threadSource) {
  const base = summarizeSource(source);
  return threadSource ? `${base}/${threadSource}` : base;
}

function collectSessionRows(dbPath, options) {
  const whereSql = options.activeOnly ? 'where archived = 0' : '';
  const sql = `
    select
      id,
      rollout_path,
      coalesce(updated_at_ms, updated_at * 1000, created_at_ms, created_at * 1000) as updated_ms,
      source,
      thread_source,
      model_provider,
      cwd,
      title,
      first_user_message,
      has_user_event,
      cli_version,
      archived
    from threads
    ${whereSql}
    order by coalesce(updated_at_ms, updated_at * 1000, created_at_ms, created_at * 1000) desc, id desc
    limit ${options.scanLimit};
  `;

  return runSqliteJson(dbPath, sql)
    .filter((row) => row.id && sourceAllowed(row, options))
    .map((row) => ({
      id: row.id,
      rolloutPath: row.rollout_path || '',
      updatedMs: Number(row.updated_ms || 0),
      source: formatSource(row.source || '?', row.thread_source || ''),
      sourceRaw: row.source || '?',
      threadSource: row.thread_source || '',
      provider: row.model_provider || '?',
      cwd: normalizeCwd(row.cwd || '?') || '?',
      rawCwd: row.cwd || '',
      sqliteTitle: normalizeText(row.title || ''),
      firstUserMessage: normalizeText(row.first_user_message || ''),
      hasUserEvent: Number(row.has_user_event || 0) === 1,
      cliVersion: row.cli_version || '?',
      archived: Number(row.archived || 0) === 1,
      nativeVisible: Number(row.archived || 0) !== 1 && normalizeText(row.first_user_message || '') !== '',
      preview: '',
      previewSource: '',
      fileState: 'unknown',
      livePath: row.rollout_path || '',
      archivedPath: '',
    }));
}

function enrichPreviews(dbPath, sessions) {
  const byId = new Map(sessions.map((session) => [session.id, session]));
  for (let i = 0; i < sessions.length; i += PREVIEW_CHUNK_SIZE) {
    const ids = sessions.slice(i, i + PREVIEW_CHUNK_SIZE).map((session) => sqliteLiteral(session.id));
    if (ids.length === 0) continue;
    const sql = `
      select
        id,
        substr(coalesce(nullif(first_user_message, ''), nullif(title, ''), ''), 1, 900) as preview
      from threads
      where id in (${ids.join(',')});
    `;
    for (const row of runSqliteJson(dbPath, sql)) {
      const session = byId.get(row.id);
      if (!session) continue;
      session.preview = normalizeText(row.preview || '');
      session.previewSource = session.preview ? 'sqlite' : '';
    }
  }
}

function sessionIdFromPath(filePath) {
  const match = String(filePath).match(/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\.jsonl$/i);
  return match ? match[1] : '';
}

function findArchiveManifests(codexHome) {
  const root = path.join(codexHome, 'archived-large-sessions');
  if (!fs.existsSync(root)) return [];
  const manifests = [];
  for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const manifestPath = path.join(root, entry.name, 'manifest.json');
    if (fs.existsSync(manifestPath)) manifests.push(manifestPath);
  }
  return manifests;
}

function loadArchiveMap(codexHome) {
  const byOriginal = new Map();
  const byId = new Map();
  const entries = [];
  for (const manifestPath of findArchiveManifests(codexHome)) {
    try {
      const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8').replace(/^\uFEFF/, ''));
      for (const entry of manifest.entries || []) {
        if (!entry.original || !entry.archived) continue;
        const original = path.resolve(entry.original);
        const archived = path.resolve(entry.archived);
        const id = sessionIdFromPath(original) || sessionIdFromPath(archived);
        entries.push({ original, archived, id });
        byOriginal.set(original.toLowerCase(), archived);
        if (id) byId.set(id, archived);
      }
    } catch (error) {
      console.error(`WARN: cannot read archive manifest ${manifestPath}: ${error.message}`);
    }
  }
  return { byOriginal, byId, entries };
}

function resolveFileState(session, archiveMap) {
  if (session.livePath && fs.existsSync(session.livePath)) {
    session.fileState = 'live';
    return;
  }
  const archivedPath = session.livePath
    ? archiveMap.byOriginal.get(path.resolve(session.livePath).toLowerCase())
    : archiveMap.byId.get(session.id);
  if (archivedPath && fs.existsSync(archivedPath)) {
    session.fileState = 'archived';
    session.archivedPath = archivedPath;
    return;
  }
  session.fileState = 'missing';
}

function textFromContent(content) {
  if (typeof content === 'string') return content;
  if (!Array.isArray(content)) return '';
  const parts = [];
  for (const part of content) {
    if (!part || typeof part !== 'object') continue;
    if (typeof part.text === 'string') parts.push(part.text);
    if (typeof part.input_text === 'string') parts.push(part.input_text);
    if (part.type === 'input_text' && typeof part.text === 'string') parts.push(part.text);
  }
  return parts.join(' ');
}

function readPrefix(filePath) {
  const fd = fs.openSync(filePath, 'r');
  try {
    const size = Math.min(fs.statSync(filePath).size, JSONL_PREFIX_BYTES);
    const buffer = Buffer.alloc(size);
    const bytesRead = fs.readSync(fd, buffer, 0, size, 0);
    return buffer.toString('utf8', 0, bytesRead);
  } finally {
    fs.closeSync(fd);
  }
}

function previewFromJsonl(session) {
  const filePath = session.fileState === 'archived' ? session.archivedPath : session.livePath;
  if (!filePath || !fs.existsSync(filePath)) return '';
  try {
    for (const line of readPrefix(filePath).split(/\r?\n/)) {
      if (!line.includes('"role":"user"') && !line.includes('"role": "user"')) continue;
      try {
        const item = JSON.parse(line);
        const content = item && item.payload && item.payload.content;
        const text = textFromContent(content);
        if (text) return normalizeText(text);
      } catch (_) {}
    }
  } catch (_) {}
  return '';
}

function fillMissingPreviewsFromJsonl(sessions) {
  for (const session of sessions) {
    if (session.preview) continue;
    const preview = previewFromJsonl(session);
    if (preview) {
      session.preview = preview;
      session.previewSource = 'jsonl';
    }
  }
}

function defaultProviderFromConfig(codexHome) {
  const configPath = path.join(codexHome, 'config.toml');
  if (!fs.existsSync(configPath)) return '';
  try {
    for (const line of fs.readFileSync(configPath, 'utf8').replace(/^\uFEFF/, '').split(/\r?\n/)) {
      const trimmed = line.trim();
      if (!trimmed || trimmed.startsWith('#')) continue;
      const match = trimmed.match(/^model_provider\s*=\s*["']([^"']+)["']/);
      if (match) return match[1];
    }
  } catch (_) {}
  return '';
}

function pushStat(stats, row) {
  const key = `${row.raw_cwd || ''}\u0000${row.model_provider || '?'}`;
  let stat = stats.get(key);
  if (!stat) {
    stat = {
      cwd: row.raw_cwd || '',
      normalizedCwd: normalizeCwd(row.raw_cwd || ''),
      provider: row.model_provider || '?',
      total: 0,
      active: 0,
      nativeVisible: 0,
      titleOnly: 0,
      hasUserEvent: 0,
    };
    stats.set(key, stat);
  }
  const archived = Number(row.archived || 0) === 1;
  const firstUserMessage = normalizeText(row.first_user_message || '');
  const title = normalizeText(row.title || '');
  stat.total += 1;
  if (!archived) stat.active += 1;
  if (!archived && firstUserMessage) stat.nativeVisible += 1;
  if (!archived && !firstUserMessage && title) stat.titleOnly += 1;
  if (!archived && Number(row.has_user_event || 0) === 1) stat.hasUserEvent += 1;
}

function collectNativeVisibilityStats(dbPath, options) {
  const sql = `
    select
      cwd as raw_cwd,
      model_provider,
      archived,
      title,
      first_user_message,
      has_user_event
    from threads
    where source in ('cli', 'vscode') or thread_source = 'user';
  `;
  const stats = new Map();
  for (const row of runSqliteJson(dbPath, sql)) {
    if (options.cwd && normalizeCwd(row.raw_cwd || '').toLowerCase() !== options.cwd.toLowerCase()) {
      continue;
    }
    pushStat(stats, row);
  }
  return Array.from(stats.values()).sort((a, b) => {
    const cwdCmp = a.normalizedCwd.localeCompare(b.normalizedCwd);
    if (cwdCmp !== 0) return cwdCmp;
    const rawCmp = a.cwd.localeCompare(b.cwd);
    if (rawCmp !== 0) return rawCmp;
    return a.provider.localeCompare(b.provider);
  });
}

function collectSessions(options) {
  const codexHome = getCodexHome();
  const dbPath = latestStateDb(codexHome);
  const archiveMap = loadArchiveMap(codexHome);
  if (!dbPath) throw new Error(`No state_*.sqlite found under ${codexHome}`);
  if (!sqliteAvailable()) throw new Error('sqlite3 is not available on PATH');

  const sessions = collectSessionRows(dbPath, options);
  enrichPreviews(dbPath, sessions);
  for (const session of sessions) resolveFileState(session, archiveMap);
  fillMissingPreviewsFromJsonl(sessions);
  return { sessions, codexHome, dbPath, archiveMap };
}

function applyFilters(sessions, options) {
  let filtered = sessions;
  if (options.cwd) {
    const expected = options.cwd.toLowerCase();
    filtered = filtered.filter((session) => session.cwd.toLowerCase() === expected);
  }
  if (options.query) {
    const terms = options.query.split(/\s+/).filter(Boolean);
    filtered = filtered.filter((session) => {
      const haystack = [
        session.id,
        session.cwd,
        session.preview,
        session.source,
        session.provider,
        session.cliVersion,
        session.fileState,
      ].join(' ').toLowerCase();
      return terms.every((term) => haystack.includes(term));
    });
  }
  return filtered;
}

function formatUpdatedTime(ms) {
  if (!Number.isFinite(ms) || ms <= 0) return '?';
  const date = new Date(ms);
  const pad = (value) => String(value).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

function printDoctor(context) {
  const historyPath = path.join(context.codexHome, 'history.jsonl');
  const sessionsPath = path.join(context.codexHome, 'sessions');
  console.log('=== Codex Session Doctor ===');
  console.log(`Picker version: ${GUARDIAN_RESUME_PICKER_VERSION}`);
  console.log(`Codex home: ${context.codexHome}`);
  console.log(`state db: ${context.dbPath}`);
  console.log(`sqlite3: ${sqliteAvailable() ? 'available' : 'missing'}`);
  console.log(`history.jsonl: ${fs.existsSync(historyPath) ? 'present' : 'missing'}`);
  console.log(`sessions/: ${fs.existsSync(sessionsPath) ? 'present' : 'missing'}`);
  console.log(`archive manifests: ${findArchiveManifests(context.codexHome).length}`);
  console.log(`archived session entries: ${context.archiveMap.entries.length}`);
  console.log(`default model_provider: ${defaultProviderFromConfig(context.codexHome) || '(not found)'}`);
  console.log('');
}

function printNativeDoctor(context, options) {
  const defaultProvider = defaultProviderFromConfig(context.codexHome);
  const stats = collectNativeVisibilityStats(context.dbPath, options);
  console.log('=== Native /resume Visibility Doctor ===');
  console.log('Native Codex 0.130.0 in-app /resume requires: archived=0, first_user_message non-empty, matching model_provider, and exact cwd when the picker is on Cwd filter.');
  console.log('Guardian native hotfix relaxes provider matching and sends both normal and \\\\?\\ cwd variants for the Cwd filter.');
  console.log(`Default config model_provider: ${defaultProvider || '(not found)'}`);
  if (options.cwd) console.log(`Normalized cwd filter: ${options.cwd}`);
  console.log('');
  if (stats.length === 0) {
    console.log('No thread rows matched the native-doctor scope.');
    console.log('');
    return;
  }
  for (const stat of stats) {
    const defaultMark = defaultProvider && stat.provider === defaultProvider ? ' default-provider' : '';
    console.log(`CWD: ${stat.cwd || '(empty)'} | Provider: ${stat.provider}${defaultMark}`);
    console.log(`    total=${stat.total} active=${stat.active} native_visible=${stat.nativeVisible} title_only=${stat.titleOnly} has_user_event=${stat.hasUserEvent}`);
  }
  console.log('');
}

function printSessions(sessions, options) {
  const visible = sessions.slice(0, options.limit);
  console.log('=== Codex Sessions With Titles ===');
  console.log(`Shown: ${visible.length} / ${sessions.length} (scan-limit=${options.scanLimit})`);
  console.log(`Policy: ${formatPolicy(options)}`);
  console.log('');
  visible.forEach((session, index) => {
    const title = session.preview ? truncate(session.preview, PREVIEW_CHARS) : '(no title found)';
    const state = session.archived ? `db-archived/${session.fileState}` : session.fileState;
    console.log(`[${index + 1}] Updated: ${formatUpdatedTime(session.updatedMs)} | ${session.id}`);
    console.log(`    CWD: ${session.cwd}`);
    console.log(`    Title: ${title}`);
    console.log(`    Native: ${session.nativeVisible ? 'yes' : 'no'} | FirstUserMessage: ${session.firstUserMessage ? 'yes' : 'no'} | HasUserEvent: ${session.hasUserEvent ? 'yes' : 'no'}`);
    console.log(`    Source: ${session.source} | Provider: ${session.provider} | CLI: ${session.cliVersion} | File: ${state}`);
    console.log(`    Open: codex resume ${session.id}`);
    console.log('');
  });
}

function formatPolicy(options) {
  const scope = options.cwd ? `cwd=${options.cwd}` : 'cross-cwd';
  const archive = options.activeOnly ? 'active-only' : 'archived-visible';
  const source = options.allSources
    ? 'all-sources'
    : options.includeExec
      ? 'cli/user+exec'
      : 'cli/user';
  return `${scope}; ${archive}; ${source}`;
}

function printableSession(session) {
  return {
    id: session.id,
    updatedMs: session.updatedMs,
    updated: formatUpdatedTime(session.updatedMs),
    cwd: session.cwd,
    title: session.preview,
    source: session.source,
    provider: session.provider,
    cliVersion: session.cliVersion,
    archived: session.archived,
    nativeVisible: session.nativeVisible,
    hasUserEvent: session.hasUserEvent,
    hasFirstUserMessage: Boolean(session.firstUserMessage),
    rawCwd: session.rawCwd,
    fileState: session.fileState,
    livePath: session.livePath,
    archivedPath: session.archivedPath,
  };
}

function findSession(sessions, target, limit) {
  if (!target) return null;
  const exact = sessions.find((session) => session.id === target);
  if (exact) return exact;
  const index = Number.parseInt(target, 10);
  if (Number.isFinite(index) && index >= 1 && index <= Math.min(limit, sessions.length)) {
    return sessions[index - 1];
  }
  return null;
}

function ensureSessionFile(session, options) {
  if (session.fileState !== 'archived') return;
  if (!session.livePath || !session.archivedPath) return;
  if (options.noRestore || options.dryRun) {
    console.log(`Would restore archived JSONL: ${session.archivedPath} -> ${session.livePath}`);
    return;
  }
  fs.mkdirSync(path.dirname(session.livePath), { recursive: true });
  fs.copyFileSync(session.archivedPath, session.livePath);
  console.log(`Restored archived JSONL: ${session.livePath}`);
}

function runResume(session, options) {
  ensureSessionFile(session, options);
  if (options.dryRun) {
    console.log(`codex resume ${session.id}`);
    return 0;
  }
  const runner = process.platform === 'win32'
    ? { command: 'cmd', args: ['/c', 'codex', 'resume', session.id] }
    : { command: 'codex', args: ['resume', session.id] };
  const result = spawnSync(runner.command, runner.args, {
    stdio: 'inherit',
    env: process.env,
    windowsHide: false,
  });
  return typeof result.status === 'number' ? result.status : 1;
}

async function promptForSession(sessions, options) {
  const visible = sessions.slice(0, options.limit);
  if (visible.length === 0) return 1;
  const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
  const answer = await new Promise((resolve) => {
    rl.question('Select session number or id (blank to cancel): ', resolve);
  });
  rl.close();
  const target = normalizeText(answer);
  if (!target) return 0;
  const session = findSession(sessions, target, options.limit);
  if (!session) {
    console.error(`No matching session for: ${target}`);
    return 1;
  }
  return runResume(session, options);
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const context = collectSessions(options);
  const filtered = applyFilters(context.sessions, options);

  if (options.doctor) printDoctor(context);
  if (options.doctor || options.nativeDoctor) printNativeDoctor(context, options);

  if (options.json) {
    console.log(JSON.stringify(filtered.slice(0, options.limit).map(printableSession), null, 2));
  } else {
    printSessions(filtered, options);
  }

  if (filtered.length === 0) {
    if (options.cwd) console.error(`No sessions matched cwd: ${options.cwd}`);
    process.exit(1);
  }

  if (options.resumeTarget) {
    const session = findSession(filtered, options.resumeTarget, options.limit);
    if (!session) {
      console.error(`No matching session for: ${options.resumeTarget}`);
      process.exit(1);
    }
    process.exit(runResume(session, options));
  }

  if (options.pick) {
    process.exit(await promptForSession(filtered, options));
  }
}

main().catch((error) => {
  console.error(`ERROR: ${error.message}`);
  process.exit(1);
});
