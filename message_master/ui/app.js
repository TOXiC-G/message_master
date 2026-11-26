const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;
const { open: openDialog } = window.__TAURI__.dialog;
const { open: openShell } = window.__TAURI__.shell;

const appWindow = getCurrentWindow();

let selectedPath = null;
let lastOutputPath = null;
let lastDisplayName = "";
let currentWhatsappUrl = "";
let updateReleaseUrl = "";
let updateInstallerPath = null;
let driveNavStack = [{ id: "root", name: "My Drive" }];
let messageCatalog = { messages: [], preachers: [], languages: ["English"] };

// --- Stage Navigation ---
const stageTabs = document.querySelectorAll(".stage-tab");
const stages = {
  record: document.getElementById("stageRecord"),
  edit: document.getElementById("stageEdit"),
  master: document.getElementById("stageMaster"),
};

function switchStage(name) {
  stageTabs.forEach((btn) => {
    const active = btn.dataset.stage === name;
    btn.classList.toggle("active", active);
    btn.setAttribute("aria-selected", active ? "true" : "false");
  });
  Object.entries(stages).forEach(([k, el]) => {
    el.classList.toggle("hidden", k !== name);
  });
  if (name === "edit") drawWaveform();
}

stageTabs.forEach((btn) => {
  btn.addEventListener("click", () => switchStage(btn.dataset.stage));
});

// Window Controls
document.getElementById("minBtn").addEventListener("click", () => appWindow.minimize());
document.getElementById("maxBtn").addEventListener("click", () => appWindow.toggleMaximize());
document.getElementById("closeBtn").addEventListener("click", () => appWindow.close());

// --- Helpers ---
function getTodayLocalISO() {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

function formatTime(sec) {
  const s = Math.max(0, sec);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const rem = Math.floor(s % 60);
  return h > 0
    ? `${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}:${String(rem).padStart(2, "0")}`
    : `${String(m).padStart(2, "0")}:${String(rem).padStart(2, "0")}.${Math.floor((s % 1) * 10)}`;
}

function formatTimerHMS(sec) {
  const s = Math.max(0, Math.floor(sec));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const rem = s % 60;
  return `${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}:${String(rem).padStart(2, "0")}`;
}

function cleanBaseName(fileName) {
  return fileName
    .replace(/\.(aup3|mp3|wav|mpeg|m4a|flac|ogg)$/i, "")
    .replace(/\.mp3$/i, "")
    .trim();
}

// --- Stage 1: Recorder & Embedded Meet ---
const micSelect = document.getElementById("micSelect");
const vuFill = document.getElementById("vuFill");
const recTimer = document.getElementById("recTimer");
const recStartBtn = document.getElementById("recStartBtn");
const recStartText = document.getElementById("recStartText");
const recPauseBtn = document.getElementById("recPauseBtn");
const recStopBtn = document.getElementById("recStopBtn");
const meetUrlInput = document.getElementById("meetUrlInput");
const openMeetBtn = document.getElementById("openMeetBtn");
const meetCamBtn = document.getElementById("meetCamBtn");
const startServiceBtn = document.getElementById("startServiceBtn");
const stopServiceBtn = document.getElementById("stopServiceBtn");
const meetStatusHint = document.getElementById("meetStatusHint");
let isRecording = false;

async function loadMics() {
  try {
    const devs = await invoke("list_audio_inputs");
    micSelect.innerHTML = "";
    for (const d of devs) {
      const opt = document.createElement("option");
      opt.value = d.name;
      opt.textContent = d.is_default ? `${d.name} (Default)` : d.name;
      if (d.is_default) opt.selected = true;
      micSelect.appendChild(opt);
    }
  } catch (e) {
    console.error("Failed to list mics:", e);
  }
}

listen("rec-level", (e) => {
  const { rms_db, elapsed_s, paused } = e.payload;
  const pct = Math.max(0, Math.min(100, ((rms_db + 60) / 60) * 100));
  vuFill.style.width = paused ? "0%" : `${pct}%`;
  recTimer.textContent = formatTimerHMS(elapsed_s);
});

async function startMicRecording() {
  if (isRecording) return;
  await invoke("rec_start", { deviceName: micSelect.value || null });
  isRecording = true;
  recStartBtn.classList.add("hidden");
  recPauseBtn.classList.remove("hidden");
  recStopBtn.classList.remove("hidden");
  recPauseBtn.textContent = "Pause";
}

async function stopMicAndEdit() {
  if (!isRecording) return;
  const wavPath = await invoke("rec_stop");
  isRecording = false;
  vuFill.style.width = "0%";
  recStartBtn.classList.remove("hidden");
  recPauseBtn.classList.add("hidden");
  recStopBtn.classList.add("hidden");
  await openFileInEditor(wavPath, `Recording_${getTodayLocalISO()}`);
  switchStage("edit");
}

recStartBtn.addEventListener("click", async () => {
  try {
    await startMicRecording();
  } catch (err) {
    meetStatusHint.textContent = String(err);
  }
});

recPauseBtn.addEventListener("click", async () => {
  const paused = await invoke("rec_pause_toggle");
  recPauseBtn.textContent = paused ? "Resume" : "Pause";
});

recStopBtn.addEventListener("click", async () => {
  try {
    await stopMicAndEdit();
  } catch (err) {
    meetStatusHint.textContent = String(err);
  }
});

async function saveMeetUrl() {
  const cfg = await invoke("get_config");
  const next = { ...cfg, meet_url: meetUrlInput.value.trim() };
  await invoke("save_config", { cfg: next });
}

meetUrlInput.addEventListener("change", saveMeetUrl);

openMeetBtn.addEventListener("click", async () => {
  await saveMeetUrl();
  try {
    await invoke("open_meet_window", { url: meetUrlInput.value.trim() });
    meetStatusHint.textContent = "Meet window opened. Sign in once if prompted, then use Start Service.";
  } catch (err) {
    meetStatusHint.textContent = String(err);
  }
});

meetCamBtn.addEventListener("click", async () => {
  try {
    await invoke("meet_control", { action: "toggle_cam" });
  } catch (err) {
    meetStatusHint.textContent = String(err);
  }
});

startServiceBtn.addEventListener("click", async () => {
  await saveMeetUrl();
  try {
    await invoke("open_meet_window", { url: meetUrlInput.value.trim() });
    await startMicRecording();
    setTimeout(() => {
      invoke("meet_control", { action: "start" }).catch(() => {});
    }, 800);
    meetStatusHint.textContent = "Service started: Mic recording + Meet camera/record triggered.";
  } catch (err) {
    meetStatusHint.textContent = String(err);
  }
});

stopServiceBtn.addEventListener("click", async () => {
  try {
    await invoke("meet_control", { action: "stop" }).catch(() => {});
    if (isRecording) {
      await stopMicAndEdit();
    }
    meetStatusHint.textContent = "Service stopped.";
  } catch (err) {
    meetStatusHint.textContent = String(err);
  }
});

// --- Stage 2: Waveform Editor ---
const waveCanvas = document.getElementById("waveCanvas");
const waveCtx = waveCanvas.getContext("2d");
const editorFileTitle = document.getElementById("editorFileTitle");
const editorTimeInfo = document.getElementById("editorTimeInfo");
const editorBrowseBtn = document.getElementById("editorBrowseBtn");
const editorStatusHint = document.getElementById("editorStatusHint");
const edPlayBtn = document.getElementById("edPlayBtn");
const edCutBtn = document.getElementById("edCutBtn");
const edCopyBtn = document.getElementById("edCopyBtn");
const edPasteBtn = document.getElementById("edPasteBtn");
const edDeleteBtn = document.getElementById("edDeleteBtn");
const edUndoBtn = document.getElementById("edUndoBtn");
const edSendToMasterBtn = document.getElementById("edSendToMasterBtn");

let waveSummary = null;
let selStartS = 0;
let selEndS = 0;
let playheadS = 0;
let isPlaying = false;
let isDraggingWave = false;
let dragAnchorS = 0;
let editorDisplayName = "";

function drawWaveform() {
  const w = waveCanvas.width;
  const h = waveCanvas.height;
  waveCtx.clearRect(0, 0, w, h);

  waveCtx.fillStyle = "#fdfaff";
  waveCtx.fillRect(0, 0, w, h);

  // Center axis
  waveCtx.strokeStyle = "#e9e0fb";
  waveCtx.lineWidth = 1;
  waveCtx.beginPath();
  waveCtx.moveTo(0, h / 2);
  waveCtx.lineTo(w, h / 2);
  waveCtx.stroke();

  if (!waveSummary || !waveSummary.peaks.length || waveSummary.duration_s <= 0) {
    waveCtx.fillStyle = "#64748b";
    waveCtx.font = "13px Plus Jakarta Sans, sans-serif";
    waveCtx.textAlign = "center";
    waveCtx.fillText("No audio loaded yet — record in Stage 1 or click Open File", w / 2, h / 2 + 4);
    return;
  }

  const dur = waveSummary.duration_s;

  // Selection highlight
  const sA = Math.min(selStartS, selEndS);
  const sB = Math.max(selStartS, selEndS);
  if (sB > sA) {
    const x1 = (sA / dur) * w;
    const x2 = (sB / dur) * w;
    waveCtx.fillStyle = "rgba(124, 58, 237, 0.16)";
    waveCtx.fillRect(x1, 0, x2 - x1, h);
  }

  // Waveform bars
  const peaks = waveSummary.peaks;
  const mid = h / 2;
  waveCtx.fillStyle = "#7c3aed";
  for (let i = 0; i < peaks.length; i++) {
    const x = (i / peaks.length) * w;
    const [mn, mx] = peaks[i];
    const yTop = mid - mx * (mid - 8);
    const yBot = mid - mn * (mid - 8);
    waveCtx.fillRect(x, yTop, Math.max(1, w / peaks.length), Math.max(1, yBot - yTop));
  }

  // Playhead
  const px = (playheadS / dur) * w;
  waveCtx.strokeStyle = "#059669";
  waveCtx.lineWidth = 2;
  waveCtx.beginPath();
  waveCtx.moveTo(px, 0);
  waveCtx.lineTo(px, h);
  waveCtx.stroke();

  const selText = sB > sA ? ` | Selection: ${formatTime(sA)} - ${formatTime(sB)}` : "";
  editorTimeInfo.textContent = `Duration: ${formatTime(dur)} | Cursor: ${formatTime(playheadS)}${selText}`;
}

function xToTime(clientX) {
  if (!waveSummary || waveSummary.duration_s <= 0) return 0;
  const rect = waveCanvas.getBoundingClientRect();
  const frac = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
  return frac * waveSummary.duration_s;
}

waveCanvas.addEventListener("mousedown", (e) => {
  if (!waveSummary) return;
  isDraggingWave = true;
  dragAnchorS = xToTime(e.clientX);
  selStartS = dragAnchorS;
  selEndS = dragAnchorS;
  playheadS = dragAnchorS;
  drawWaveform();
});

window.addEventListener("mousemove", (e) => {
  if (!isDraggingWave) return;
  selEndS = xToTime(e.clientX);
  drawWaveform();
});

window.addEventListener("mouseup", () => {
  isDraggingWave = false;
});

async function openFileInEditor(path, overrideName) {
  editorStatusHint.textContent = "Loading waveform...";
  try {
    waveSummary = await invoke("editor_load", { inputPath: path });
    selStartS = 0;
    selEndS = 0;
    playheadS = 0;
    const fname = path.split(/[\\/]/).pop();
    editorDisplayName = overrideName || cleanBaseName(fname);
    editorFileTitle.textContent = editorDisplayName;
    editorStatusHint.textContent = "Ready. Drag to select, Cut/Copy/Paste, then click Master This Audio.";
    drawWaveform();
  } catch (err) {
    editorStatusHint.textContent = String(err);
  }
}

editorBrowseBtn.addEventListener("click", async () => {
  const file = await openDialog({
    multiple: false,
    filters: [{ name: "Audio", extensions: ["aup3", "mp3", "wav", "mpeg", "m4a", "flac", "ogg"] }],
  });
  if (file) await openFileInEditor(file);
});

listen("playback-pos", (e) => {
  playheadS = e.payload.current_s;
  isPlaying = e.payload.playing;
  edPlayBtn.textContent = isPlaying ? "Stop" : "Play";
  drawWaveform();
});

edPlayBtn.addEventListener("click", async () => {
  if (!waveSummary) return;
  if (isPlaying) {
    await invoke("editor_stop");
  } else {
    await invoke("editor_play", { startS: playheadS });
  }
});

async function applyEditSummary(sum, msg) {
  waveSummary = sum;
  selStartS = 0;
  selEndS = 0;
  if (playheadS > sum.duration_s) playheadS = sum.duration_s;
  editorStatusHint.textContent = msg;
  drawWaveform();
}

edCutBtn.addEventListener("click", async () => {
  try {
    const sum = await invoke("editor_cut", { startS: selStartS, endS: selEndS });
    await applyEditSummary(sum, "Cut selection to clipboard.");
  } catch (e) { editorStatusHint.textContent = String(e); }
});

edCopyBtn.addEventListener("click", async () => {
  try {
    const sum = await invoke("editor_copy", { startS: selStartS, endS: selEndS });
    await applyEditSummary(sum, "Copied selection to clipboard.");
  } catch (e) { editorStatusHint.textContent = String(e); }
});

edPasteBtn.addEventListener("click", async () => {
  try {
    const sum = await invoke("editor_paste", { atS: playheadS });
    await applyEditSummary(sum, "Pasted clipboard at cursor.");
  } catch (e) { editorStatusHint.textContent = String(e); }
});

edDeleteBtn.addEventListener("click", async () => {
  try {
    const sum = await invoke("editor_delete", { startS: selStartS, endS: selEndS });
    await applyEditSummary(sum, "Deleted selection.");
  } catch (e) { editorStatusHint.textContent = String(e); }
});

edUndoBtn.addEventListener("click", async () => {
  try {
    const sum = await invoke("editor_undo");
    await applyEditSummary(sum, "Undid last edit.");
  } catch (e) { editorStatusHint.textContent = String(e); }
});

edSendToMasterBtn.addEventListener("click", async () => {
  if (!waveSummary) return;
  try {
    await invoke("editor_stop");
    const wavPath = await invoke("editor_export_wav");
    setSelectedFile(wavPath, editorDisplayName);
    switchStage("master");
  } catch (e) {
    editorStatusHint.textContent = String(e);
  }
});

window.addEventListener("keydown", (e) => {
  if (stages.edit.classList.contains("hidden")) return;
  if (document.activeElement && ["INPUT", "TEXTAREA", "SELECT"].includes(document.activeElement.tagName)) return;
  if (e.ctrlKey && e.key.toLowerCase() === "x") { e.preventDefault(); edCutBtn.click(); }
  else if (e.ctrlKey && e.key.toLowerCase() === "c") { e.preventDefault(); edCopyBtn.click(); }
  else if (e.ctrlKey && e.key.toLowerCase() === "v") { e.preventDefault(); edPasteBtn.click(); }
  else if (e.ctrlKey && e.key.toLowerCase() === "z") { e.preventDefault(); edUndoBtn.click(); }
  else if (e.key === "Delete") { e.preventDefault(); edDeleteBtn.click(); }
  else if (e.key === " ") { e.preventDefault(); edPlayBtn.click(); }
});

// --- Stage 3: Message Name Builder & Autocomplete ---
const metaDate = document.getElementById("metaDate");
const metaTitle = document.getElementById("metaTitle");
const metaPart = document.getElementById("metaPart");
const metaPreacher = document.getElementById("metaPreacher");
const metaLanguage = document.getElementById("metaLanguage");
const titleDropdown = document.getElementById("titleDropdown");
const preacherList = document.getElementById("preacherList");
const languageList = document.getElementById("languageList");
const toggleBuilderBtn = document.getElementById("toggleBuilderBtn");
const builderGrid = document.getElementById("builderGrid");
const customNameInput = document.getElementById("customNameInput");
const driveNameInput = document.getElementById("driveNameInput");

let rawEditMode = false;

function rebuildFormattedName() {
  if (rawEditMode) return;
  const iso = metaDate.value || getTodayLocalISO();
  const [y, m, d] = iso.split("-");
  const dateStr = `${d}.${m}.${y}`;
  const title = metaTitle.value.trim();
  const part = metaPart.value.trim();
  const preacher = metaPreacher.value.trim();
  const lang = metaLanguage.value.trim();

  if (!title) return;
  const pieces = [dateStr, title];
  if (part) pieces.push(`Part ${part}`);
  if (preacher) pieces.push(preacher);
  if (lang) pieces.push(lang);

  const built = pieces.join(" - ");
  customNameInput.value = built;
  driveNameInput.value = built;
}

[metaDate, metaTitle, metaPart, metaPreacher, metaLanguage].forEach((el) => {
  el.addEventListener("input", rebuildFormattedName);
});

toggleBuilderBtn.addEventListener("click", () => {
  rawEditMode = !rawEditMode;
  builderGrid.classList.toggle("hidden", rawEditMode);
  toggleBuilderBtn.textContent = rawEditMode ? "Use Builder" : "Raw Edit";
});

metaTitle.addEventListener("input", () => {
  const q = metaTitle.value.trim().toLowerCase();
  const matches = messageCatalog.messages.filter((m) =>
    !q ? true : m.title.toLowerCase().includes(q)
  );
  if (!q || matches.length === 0) {
    titleDropdown.classList.add("hidden");
    return;
  }
  titleDropdown.innerHTML = "";
  for (const item of matches.slice(0, 6)) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "auto-item";
    btn.innerHTML = `<span></span><span class="auto-meta"></span>`;
    btn.children[0].textContent = item.title;
    btn.children[1].textContent = `Next: Part ${(item.last_part || 0) + 1}${item.preacher ? " • " + item.preacher : ""}`;
    btn.addEventListener("click", () => {
      metaTitle.value = item.title;
      metaPart.value = String((item.last_part || 0) + 1);
      if (item.preacher) metaPreacher.value = item.preacher;
      if (item.language) metaLanguage.value = item.language;
      titleDropdown.classList.add("hidden");
      rebuildFormattedName();
    });
    titleDropdown.appendChild(btn);
  }
  titleDropdown.classList.remove("hidden");
});

document.addEventListener("click", (e) => {
  if (!metaTitle.contains(e.target) && !titleDropdown.contains(e.target)) {
    titleDropdown.classList.add("hidden");
  }
});

async function refreshCatalog() {
  try {
    messageCatalog = await invoke("get_catalog");
    preacherList.innerHTML = "";
    for (const p of messageCatalog.preachers) {
      const opt = document.createElement("option");
      opt.value = p;
      preacherList.appendChild(opt);
    }
    languageList.innerHTML = "";
    for (const l of messageCatalog.languages) {
      const opt = document.createElement("option");
      opt.value = l;
      languageList.appendChild(opt);
    }
  } catch (e) {
    console.error(e);
  }
}

// --- Stage 3: Master & Share ---
const dropZone = document.getElementById("dropZone");
const dropTitle = document.getElementById("dropTitle");
const dropSub = document.getElementById("dropSub");
const localOutputLabel = document.getElementById("localOutputLabel");
const pickLocalDirBtn = document.getElementById("pickLocalDirBtn");
const modeDirectBtn = document.getElementById("modeDirectBtn");
const modeBaseBtn = document.getElementById("modeBaseBtn");
const saveDateInput = document.getElementById("saveDateInput");
const basePathHint = document.getElementById("basePathHint");
const localDirTitle = document.getElementById("localDirTitle");
const destFolderName = document.getElementById("destFolderName");
const browseDriveBtn = document.getElementById("browseDriveBtn");
const startBtn = document.getElementById("startBtn");
const startBtnText = document.getElementById("startBtnText");
const uploadDirectlyToggle = document.getElementById("uploadDirectlyToggle");
const preDriveSlot = document.getElementById("preDriveSlot");
const postDriveSlot = document.getElementById("postDriveSlot");
const driveConfigGroup = document.getElementById("driveConfigGroup");
const driveStatusDot = document.getElementById("driveStatusDot");
const driveConnLabel = document.getElementById("driveConnLabel");
const progressCard = document.getElementById("progressCard");
const progressLabel = document.getElementById("progressLabel");
const progressStep = document.getElementById("progressStep");
const progressFill = document.getElementById("progressFill");
const errorBanner = document.getElementById("errorBanner");
const resultCard = document.getElementById("resultCard");
const shareTitle = document.getElementById("shareTitle");
const shareMessage = document.getElementById("shareMessage");
const uploadDriveBtn = document.getElementById("uploadDriveBtn");
const uploadDriveBtnText = document.getElementById("uploadDriveBtnText");
const copyBtn = document.getElementById("copyBtn");
const copyBtnText = document.getElementById("copyBtnText");
const whatsappBtn = document.getElementById("whatsappBtn");
const driveWarn = document.getElementById("driveWarn");
const driveNameField = document.getElementById("driveNameField");
const driveDestPanel = document.getElementById("driveDestPanel");
const driveModeDirectBtn = document.getElementById("driveModeDirectBtn");
const driveModeBaseBtn = document.getElementById("driveModeBaseBtn");
const driveDateInput = document.getElementById("driveDateInput");
const driveBasePathHint = document.getElementById("driveBasePathHint");
const driveDirTitle = document.getElementById("driveDirTitle");

// Drive Browser Modal
const driveModal = document.getElementById("driveModal");
const closeDriveModalBtn = document.getElementById("closeDriveModalBtn");
const driveBreadcrumbs = document.getElementById("driveBreadcrumbs");
const driveFolderList = document.getElementById("driveFolderList");
const newFolderInput = document.getElementById("newFolderInput");
const createFolderBtn = document.getElementById("createFolderBtn");
const selectFolderBtn = document.getElementById("selectFolderBtn");
const driveBrowserStatus = document.getElementById("driveBrowserStatus");

// Settings Modal
const settingsModal = document.getElementById("settingsModal");
const openSettingsBtn = document.getElementById("openSettingsBtn");
const closeSettingsBtn = document.getElementById("closeSettingsBtn");
const saveSettingsBtn = document.getElementById("saveSettingsBtn");
const connectDriveBtn = document.getElementById("connectDriveBtn");
const oauthStatus = document.getElementById("oauthStatus");
const checkUpdatesBtn = document.getElementById("checkUpdatesBtn");
const updateCheckStatus = document.getElementById("updateCheckStatus");

function updateSaveModeUI(mode) {
  const isBase = mode === "base";
  modeDirectBtn.classList.toggle("active", !isBase);
  modeBaseBtn.classList.toggle("active", isBase);
  saveDateInput.classList.toggle("hidden", !isBase);
  basePathHint.classList.toggle("hidden", !isBase);
  localDirTitle.textContent = isBase ? "Base Folder (Year / Month)" : "Save to Local Folder";
  if (isBase && saveDateInput.value) {
    const [y, m] = saveDateInput.value.split("-").map(Number);
    const dt = new Date(y, m - 1, 1);
    const monthName = dt.toLocaleString("en-US", { month: "long" });
    basePathHint.textContent = `Saves into: <base>\\${y}\\${m}. ${monthName}\\`;
  }
}

function updateDriveModeUI(mode) {
  const isBase = mode === "base";
  driveModeDirectBtn.classList.toggle("active", !isBase);
  driveModeBaseBtn.classList.toggle("active", isBase);
  driveDateInput.classList.toggle("hidden", !isBase);
  driveBasePathHint.classList.toggle("hidden", !isBase);
  driveDirTitle.textContent = isBase ? "Drive Base Folder (Year / Month)" : "Google Drive Destination";
  if (isBase && driveDateInput.value) {
    const [y, m] = driveDateInput.value.split("-").map(Number);
    const dt = new Date(y, m - 1, 1);
    const monthName = dt.toLocaleString("en-US", { month: "long" });
    driveBasePathHint.textContent = `Uploads into: <Drive Folder> / ${y} / ${m}. ${monthName} /`;
  }
}

function updateUploadDirectlyUI(direct) {
  uploadDirectlyToggle.checked = direct;
  if (direct) {
    preDriveSlot.appendChild(driveConfigGroup);
    driveConfigGroup.classList.remove("hidden");
    startBtnText.textContent = "Process & Upload";
  } else {
    postDriveSlot.appendChild(driveConfigGroup);
    driveConfigGroup.classList.remove("hidden");
    startBtnText.textContent = "Process & Save";
  }
}

function applyConfigToUI(cfg) {
  document.getElementById("cfgFolder").value = cfg.drive_folder_input || "";
  document.getElementById("cfgClientId").value = cfg.oauth_client_id || "";
  document.getElementById("cfgClientSecret").value = cfg.oauth_client_secret || "";
  document.getElementById("cfgMaxMb").value = cfg.max_size_mb ?? 30;
  document.getElementById("cfgLufs").value = cfg.target_lufs ?? -16;
  document.getElementById("cfgPublicLink").checked = cfg.make_public_link ?? true;
  meetUrlInput.value = cfg.meet_url || "";

  if (cfg.drive_folder_name) destFolderName.textContent = cfg.drive_folder_name;
  else if (cfg.drive_folder_input) destFolderName.textContent = cfg.drive_folder_input;
  else destFolderName.textContent = "My Drive (Root)";

  if (cfg.local_output_dir) {
    localOutputLabel.textContent = cfg.local_output_dir.split(/[\\/]/).pop() || cfg.local_output_dir;
    localOutputLabel.title = cfg.local_output_dir;
  } else {
    localOutputLabel.textContent = "Same as input file";
    localOutputLabel.title = "";
  }

  updateSaveModeUI(cfg.save_mode || "direct");
  updateDriveModeUI(cfg.drive_save_mode || "direct");
  updateUploadDirectlyUI(Boolean(cfg.upload_directly));

  const connected = Boolean(cfg.refresh_token);
  driveStatusDot.classList.toggle("connected", connected);
  driveConnLabel.textContent = connected ? "Google Drive connected" : "Google Drive not connected";
  oauthStatus.textContent = connected ? "Connected to Google Drive" : "Not connected";
  oauthStatus.classList.toggle("connected", connected);
}

function readConfigFromUI(existing) {
  return {
    drive_folder_input: document.getElementById("cfgFolder").value.trim(),
    drive_folder_name: existing.drive_folder_name || "",
    local_output_dir: existing.local_output_dir || "",
    save_mode: existing.save_mode || "direct",
    drive_save_mode: existing.drive_save_mode || "direct",
    upload_directly: existing.upload_directly ?? false,
    meet_url: meetUrlInput.value.trim(),
    oauth_client_id: document.getElementById("cfgClientId").value.trim(),
    oauth_client_secret: document.getElementById("cfgClientSecret").value.trim(),
    refresh_token: existing.refresh_token,
    max_size_mb: parseFloat(document.getElementById("cfgMaxMb").value) || 30.0,
    target_lufs: parseFloat(document.getElementById("cfgLufs").value) || -16.0,
    make_public_link: document.getElementById("cfgPublicLink").checked,
  };
}

function setSelectedFile(path, overrideDisplay) {
  selectedPath = path;
  const fileName = path.split(/[\\/]/).pop();
  dropTitle.textContent = overrideDisplay || fileName;
  dropSub.textContent = path;
  const cleaned = overrideDisplay || cleanBaseName(fileName);
  if (!metaTitle.value.trim()) {
    customNameInput.value = cleaned;
    driveNameInput.value = cleaned;
  } else {
    rebuildFormattedName();
  }
  startBtn.disabled = false;
  errorBanner.classList.add("hidden");
}

async function pickFile() {
  const file = await openDialog({
    multiple: false,
    filters: [{ name: "Audacity & Audio Files", extensions: ["aup3", "mp3", "wav", "mpeg", "m4a", "flac", "ogg"] }],
  });
  if (file) setSelectedFile(file);
}

dropZone.addEventListener("click", pickFile);
dropZone.addEventListener("keydown", (e) => {
  if (e.key === "Enter" || e.key === " ") {
    e.preventDefault();
    pickFile();
  }
});

listen("tauri://drag-enter", () => dropZone.classList.add("dragover"));
listen("tauri://drag-leave", () => dropZone.classList.remove("dragover"));
listen("tauri://drag-drop", (event) => {
  dropZone.classList.remove("dragover");
  const paths = event.payload?.paths || [];
  if (paths.length > 0) {
    if (!stages.edit.classList.contains("hidden")) {
      openFileInEditor(paths[0]);
    } else {
      setSelectedFile(paths[0]);
    }
  }
});

pickLocalDirBtn.addEventListener("click", async () => {
  const dir = await openDialog({ directory: true, multiple: false });
  if (dir) {
    const existing = await invoke("get_config");
    const updated = { ...existing, local_output_dir: dir };
    await invoke("save_config", { cfg: updated });
    applyConfigToUI(updated);
  }
});

async function setSaveMode(mode) {
  const existing = await invoke("get_config");
  const updated = { ...existing, save_mode: mode };
  await invoke("save_config", { cfg: updated });
  updateSaveModeUI(mode);
}

modeDirectBtn.addEventListener("click", () => setSaveMode("direct"));
modeBaseBtn.addEventListener("click", () => setSaveMode("base"));
saveDateInput.addEventListener("change", async () => {
  const cfg = await invoke("get_config");
  updateSaveModeUI(cfg.save_mode || "direct");
  if (saveDateInput.value) {
    driveDateInput.value = saveDateInput.value;
    updateDriveModeUI(cfg.drive_save_mode || "direct");
  }
});

async function setDriveMode(mode) {
  const existing = await invoke("get_config");
  const updated = { ...existing, drive_save_mode: mode };
  await invoke("save_config", { cfg: updated });
  updateDriveModeUI(mode);
}

driveModeDirectBtn.addEventListener("click", () => setDriveMode("direct"));
driveModeBaseBtn.addEventListener("click", () => setDriveMode("base"));
driveDateInput.addEventListener("change", async () => {
  const cfg = await invoke("get_config");
  updateDriveModeUI(cfg.drive_save_mode || "direct");
});

uploadDirectlyToggle.addEventListener("change", async () => {
  const existing = await invoke("get_config");
  const updated = { ...existing, upload_directly: uploadDirectlyToggle.checked };
  await invoke("save_config", { cfg: updated });
  updateUploadDirectlyUI(uploadDirectlyToggle.checked);
});

listen("mastering-progress", (event) => {
  const { step, label } = event.payload;
  progressCard.classList.remove("hidden");
  progressLabel.textContent = label;
  progressStep.textContent = `Step ${step} / 5`;
  progressFill.style.width = `${Math.min(100, Math.round((step / 5) * 100))}%`;
});

listen("update-available", (event) => {
  const info = event.payload;
  const badge = document.getElementById("updateBadge");
  updateReleaseUrl = info.release_url;
  updateInstallerPath = info.installer_path || null;
  document.getElementById("updateText").textContent = updateInstallerPath
    ? `Restart to update (v${info.latest_version})`
    : `Update available (v${info.latest_version})`;
  badge.classList.remove("hidden");
});

document.getElementById("updateBadge").addEventListener("click", async () => {
  if (updateInstallerPath) {
    await invoke("install_update_and_restart", { installerPath: updateInstallerPath });
  } else if (updateReleaseUrl) {
    openShell(updateReleaseUrl);
  }
});

startBtn.addEventListener("click", async () => {
  if (!selectedPath) return;
  startBtn.disabled = true;
  errorBanner.classList.add("hidden");
  resultCard.classList.add("hidden");
  progressCard.classList.remove("hidden");
  progressFill.style.width = "10%";
  progressStep.textContent = "Step 1 / 5";
  progressLabel.textContent = "Starting mastering pipeline...";

  if (metaTitle.value.trim()) {
    await invoke("save_catalog_entry", {
      title: metaTitle.value.trim(),
      part: parseInt(metaPart.value, 10) || 0,
      preacher: metaPreacher.value.trim(),
      language: metaLanguage.value.trim(),
    });
    refreshCatalog();
  }

  try {
    const cfg = await invoke("get_config");
    const res = await invoke("process_audio", {
      inputPath: selectedPath,
      customName: customNameInput.value.trim() || null,
      outputDir: cfg.local_output_dir || null,
      saveMode: cfg.save_mode || "direct",
      saveDate: saveDateInput.value || null,
    });

    progressCard.classList.add("hidden");
    resultCard.classList.remove("hidden");

    document.getElementById("mClicks").textContent = res.clicks_repaired;
    document.getElementById("mSilences").textContent = `${res.seconds_trimmed.toFixed(1)}s`;
    document.getElementById("mSize").textContent = `${res.output_size_mb.toFixed(2)} MB`;
    document.getElementById("mBitrate").textContent = `${res.bitrate_kbps} kbps MP3`;
    document.getElementById("mLufs").textContent = `${res.final_lufs.toFixed(1)} LUFS`;

    lastOutputPath = res.output_path;
    lastDisplayName = res.display_name;
    if (!driveNameInput.value.trim()) driveNameInput.value = res.display_name;

    if (uploadDirectlyToggle.checked) {
      progressCard.classList.remove("hidden");
      resultCard.classList.add("hidden");
      progressStep.textContent = "Uploading";
      progressLabel.textContent = "Uploading mastered MP3 to Google Drive...";
      progressFill.style.width = "100%";

      try {
        const upRes = await invoke("upload_mastered_file", {
          filePath: lastOutputPath,
          displayName: lastDisplayName,
          driveName: driveNameInput.value.trim() || null,
          driveMode: cfg.drive_save_mode || "direct",
          saveDate: driveDateInput.value || null,
        });
        progressCard.classList.add("hidden");
        resultCard.classList.remove("hidden");
        shareTitle.textContent = "Uploaded to Google Drive — Ready to Share";
        shareMessage.value = upRes.share_message;
        currentWhatsappUrl = upRes.whatsapp_url;
        driveWarn.classList.add("hidden");
        uploadDriveBtn.classList.add("hidden");
        whatsappBtn.classList.remove("hidden");
      } catch (upErr) {
        progressCard.classList.add("hidden");
        resultCard.classList.remove("hidden");
        shareTitle.textContent = "Saved to Disk — Upload Failed";
        shareMessage.value = res.share_message;
        currentWhatsappUrl = res.whatsapp_url;
        driveWarn.textContent = `Upload failed: ${String(upErr)}`;
        driveWarn.classList.remove("hidden");
        postDriveSlot.appendChild(driveConfigGroup);
        driveConfigGroup.classList.remove("hidden");
        uploadDriveBtn.classList.remove("hidden");
        uploadDriveBtn.disabled = false;
        uploadDriveBtnText.textContent = "Retry Upload";
        whatsappBtn.classList.add("hidden");
      }
    } else {
      postDriveSlot.appendChild(driveConfigGroup);
      driveConfigGroup.classList.remove("hidden");
      driveDestPanel.classList.remove("hidden");
      driveNameField.classList.remove("hidden");
      driveNameInput.value = res.display_name;
      shareTitle.textContent = "Saved to Disk — Choose Drive Folder & Upload";
      shareMessage.value = res.share_message;
      currentWhatsappUrl = res.whatsapp_url;
      driveWarn.classList.add("hidden");

      uploadDriveBtn.classList.remove("hidden");
      uploadDriveBtn.disabled = false;
      uploadDriveBtnText.textContent = "Upload to Google Drive";
      whatsappBtn.classList.add("hidden");
    }
  } catch (err) {
    progressCard.classList.add("hidden");
    errorBanner.textContent = String(err);
    errorBanner.classList.remove("hidden");
  } finally {
    startBtn.disabled = false;
  }
});

uploadDriveBtn.addEventListener("click", async () => {
  if (!lastOutputPath) return;
  uploadDriveBtn.disabled = true;
  uploadDriveBtnText.textContent = "Uploading...";
  driveWarn.classList.add("hidden");

  try {
    const cfg = await invoke("get_config");
    const res = await invoke("upload_mastered_file", {
      filePath: lastOutputPath,
      displayName: lastDisplayName,
      driveName: driveNameInput.value.trim() || null,
      driveMode: cfg.drive_save_mode || "direct",
      saveDate: driveDateInput.value || null,
    });
    shareTitle.textContent = "Uploaded to Google Drive — Ready to Share";
    shareMessage.value = res.share_message;
    currentWhatsappUrl = res.whatsapp_url;
    if (!uploadDirectlyToggle.checked) driveConfigGroup.classList.add("hidden");
    uploadDriveBtn.classList.add("hidden");
    whatsappBtn.classList.remove("hidden");
  } catch (err) {
    driveWarn.textContent = `Upload failed: ${String(err)}`;
    driveWarn.classList.remove("hidden");
    uploadDriveBtn.disabled = false;
    uploadDriveBtnText.textContent = "Retry Upload";
  }
});

copyBtn.addEventListener("click", async () => {
  await navigator.clipboard.writeText(shareMessage.value);
  copyBtnText.textContent = "Copied!";
  setTimeout(() => { copyBtnText.textContent = "Copy Message"; }, 1800);
});

whatsappBtn.addEventListener("click", () => {
  if (currentWhatsappUrl) openShell(currentWhatsappUrl);
});

// --- Drive Folder Browser ---
async function renderDriveFolder(parentId) {
  driveBrowserStatus.textContent = "";
  driveFolderList.innerHTML = `<div class="folder-empty">Loading folders...</div>`;
  renderBreadcrumbs();

  try {
    const folders = await invoke("list_drive_folders", { parentId });
    if (folders.length === 0) {
      driveFolderList.innerHTML = `<div class="folder-empty">No subfolders here. You can use this folder or create one below.</div>`;
      return;
    }
    driveFolderList.innerHTML = "";
    for (const f of folders) {
      const row = document.createElement("div");
      row.className = "folder-row";
      const btn = document.createElement("button");
      btn.className = "folder-item";
      btn.type = "button";
      btn.innerHTML = `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="#7c3aed" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path></svg><span></span>`;
      btn.querySelector("span").textContent = f.name;
      btn.addEventListener("click", () => {
        driveNavStack.push({ id: f.id, name: f.name });
        renderDriveFolder(f.id);
      });

      const actions = document.createElement("div");
      actions.className = "folder-actions";

      const renameBtn = document.createElement("button");
      renameBtn.className = "folder-action-btn";
      renameBtn.type = "button";
      renameBtn.title = "Rename folder";
      renameBtn.innerHTML = `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"></path><path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z"></path></svg>`;
      renameBtn.addEventListener("click", async (e) => {
        e.stopPropagation();
        const nextName = window.prompt("Rename folder to:", f.name);
        if (!nextName || !nextName.trim() || nextName.trim() === f.name) return;
        driveBrowserStatus.textContent = "Renaming...";
        try {
          await invoke("rename_drive_folder", { folderId: f.id, newName: nextName.trim() });
          await renderDriveFolder(driveNavStack[driveNavStack.length - 1].id);
          driveBrowserStatus.textContent = `Renamed to "${nextName.trim()}"`;
        } catch (err) { driveBrowserStatus.textContent = String(err); }
      });

      const deleteBtn = document.createElement("button");
      deleteBtn.className = "folder-action-btn danger";
      deleteBtn.type = "button";
      deleteBtn.title = "Move folder to Trash";
      deleteBtn.innerHTML = `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"></polyline><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"></path><path d="M10 11v6"></path><path d="M14 11v6"></path></svg>`;
      deleteBtn.addEventListener("click", async (e) => {
        e.stopPropagation();
        if (!window.confirm(`Move "${f.name}" to Google Drive Trash?`)) return;
        driveBrowserStatus.textContent = "Moving to Trash...";
        try {
          await invoke("delete_drive_folder", { folderId: f.id });
          await renderDriveFolder(driveNavStack[driveNavStack.length - 1].id);
          driveBrowserStatus.textContent = `Moved "${f.name}" to Trash`;
        } catch (err) { driveBrowserStatus.textContent = String(err); }
      });

      actions.appendChild(renameBtn);
      actions.appendChild(deleteBtn);
      row.appendChild(btn);
      row.appendChild(actions);
      driveFolderList.appendChild(row);
    }
  } catch (err) {
    driveFolderList.innerHTML = `<div class="folder-empty" style="color:#dc2626;">${String(err)}</div>`;
  }
}

function renderBreadcrumbs() {
  driveBreadcrumbs.innerHTML = "";
  driveNavStack.forEach((item, idx) => {
    if (idx > 0) {
      const sep = document.createElement("span");
      sep.textContent = "/";
      sep.style.color = "#94a3b8";
      driveBreadcrumbs.appendChild(sep);
    }
    const b = document.createElement("button");
    b.className = "crumb-btn";
    b.type = "button";
    b.textContent = item.name;
    b.addEventListener("click", () => {
      driveNavStack = driveNavStack.slice(0, idx + 1);
      renderDriveFolder(item.id);
    });
    driveBreadcrumbs.appendChild(b);
  });
}

browseDriveBtn.addEventListener("click", () => {
  driveNavStack = [{ id: "root", name: "My Drive" }];
  driveModal.classList.remove("hidden");
  renderDriveFolder("root");
});

closeDriveModalBtn.addEventListener("click", () => driveModal.classList.add("hidden"));

createFolderBtn.addEventListener("click", async () => {
  const name = newFolderInput.value.trim();
  if (!name) return;
  const current = driveNavStack[driveNavStack.length - 1];
  driveBrowserStatus.textContent = "Creating folder...";
  try {
    const created = await invoke("create_drive_folder", { parentId: current.id, name });
    newFolderInput.value = "";
    driveBrowserStatus.textContent = `Created "${created.name}"`;
    driveNavStack.push({ id: created.id, name: created.name });
    await renderDriveFolder(created.id);
  } catch (err) { driveBrowserStatus.textContent = String(err); }
});

selectFolderBtn.addEventListener("click", async () => {
  const current = driveNavStack[driveNavStack.length - 1];
  const pathLabel = driveNavStack.map((x) => x.name).join(" / ");
  const existing = await invoke("get_config");
  const updated = {
    ...existing,
    drive_folder_input: current.id === "root" ? "" : current.id,
    drive_folder_name: pathLabel,
  };
  await invoke("save_config", { cfg: updated });
  applyConfigToUI(updated);
  driveModal.classList.add("hidden");
});

// --- Settings Modal ---
openSettingsBtn.addEventListener("click", async () => {
  const cfg = await invoke("get_config");
  applyConfigToUI(cfg);
  settingsModal.classList.remove("hidden");
});

closeSettingsBtn.addEventListener("click", () => settingsModal.classList.add("hidden"));

saveSettingsBtn.addEventListener("click", async () => {
  const existing = await invoke("get_config");
  const updated = readConfigFromUI(existing);
  if (updated.drive_folder_input !== existing.drive_folder_input) {
    updated.drive_folder_name = updated.drive_folder_input ? updated.drive_folder_input : "My Drive (Root)";
  }
  await invoke("save_config", { cfg: updated });
  applyConfigToUI(updated);
  settingsModal.classList.add("hidden");
});

connectDriveBtn.addEventListener("click", async () => {
  const existing = await invoke("get_config");
  const cfg = readConfigFromUI(existing);
  oauthStatus.textContent = "Waiting for browser sign-in...";
  try {
    const msg = await invoke("connect_google_drive", { cfg });
    const refreshed = await invoke("get_config");
    applyConfigToUI(refreshed);
    oauthStatus.textContent = msg;
  } catch (err) {
    oauthStatus.textContent = String(err);
    oauthStatus.classList.remove("connected");
  }
});

checkUpdatesBtn.addEventListener("click", async () => {
  updateCheckStatus.textContent = "Checking & downloading...";
  updateCheckStatus.classList.remove("connected");
  try {
    const info = await invoke("check_updates_now");
    if (info) {
      updateCheckStatus.classList.add("connected");
      updateCheckStatus.textContent = info.installer_path
        ? `v${info.latest_version} ready — click the green pill in the title bar to restart & install.`
        : `Update available: v${info.latest_version}`;
    } else {
      const ver = await invoke("get_app_version");
      updateCheckStatus.textContent = `You're up to date (v${ver})`;
    }
  } catch (err) {
    updateCheckStatus.textContent = "Could not check for updates.";
  }
});

async function init() {
  const today = getTodayLocalISO();
  saveDateInput.value = today;
  driveDateInput.value = today;
  metaDate.value = today;
  try {
    const ver = await invoke("get_app_version");
    document.getElementById("appVersion").textContent = `v${ver}`;
    const cfg = await invoke("get_config");
    applyConfigToUI(cfg);
    await refreshCatalog();
    await loadMics();
  } catch (e) {
    console.error(e);
  }
}

init();
