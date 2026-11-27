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

let currentStage = "master";

function switchStage(name) {
  currentStage = name;
  stageTabs.forEach((btn) => {
    const active = btn.dataset.stage === name;
    btn.classList.toggle("active", active);
    btn.setAttribute("aria-selected", active ? "true" : "false");
  });
  Object.entries(stages).forEach(([k, el]) => {
    el.classList.toggle("hidden", k !== name);
  });
  if (name === "edit") drawWaveform();
  if (name === "record" && !isRecording) {
    startLiveMonitor();
  } else if (name !== "record" && !isRecording) {
    invoke("rec_monitor_stop").catch(() => {});
  }
  if (typeof syncMeetBounds === "function") {
    requestAnimationFrame(() => syncMeetBounds());
  }
  if (name === "record" && typeof meetLoaded !== "undefined" && !meetLoaded && cfgMeetUrl && cfgMeetUrl.value.trim()) {
    requestAnimationFrame(() => loadEmbeddedMeet());
  }
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
const micGainSlider = document.getElementById("micGainSlider");
const micGainLabel = document.getElementById("micGainLabel");
const recMonitorStatus = document.getElementById("recMonitorStatus");
const recDbReadout = document.getElementById("recDbReadout");
const liveWaveCanvas = document.getElementById("liveWaveCanvas");
const liveWaveCtx = liveWaveCanvas ? liveWaveCanvas.getContext("2d") : null;
const meetEmbedHost = document.getElementById("meetEmbedHost");
const recTimer = document.getElementById("recTimer");
const recStartBtn = document.getElementById("recStartBtn");
const recStartText = document.getElementById("recStartText");
const recPauseBtn = document.getElementById("recPauseBtn");
const recClearBtn = document.getElementById("recClearBtn");
const recStopBtn = document.getElementById("recStopBtn");
const openMeetBtn = document.getElementById("openMeetBtn");
const meetCamBtn = document.getElementById("meetCamBtn");
const startServiceBtn = document.getElementById("startServiceBtn");
const stopServiceBtn = document.getElementById("stopServiceBtn");
const meetStatusHint = document.getElementById("meetStatusHint");
let isRecording = false;
let originalRecordedWavPath = null;

async function startLiveMonitor() {
  if (isRecording) return;
  try {
    await invoke("rec_monitor_start", { deviceName: micSelect.value || null });
    recMonitorStatus.textContent = "Live Monitor";
    recMonitorStatus.classList.remove("recording", "paused");
  } catch (e) {
    console.warn("Mic monitor unavailable:", e);
  }
}

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
    if (currentStage === "record" && !isRecording) {
      await startLiveMonitor();
    }
  } catch (e) {
    console.error("Failed to list mics:", e);
  }
}

micSelect.addEventListener("change", () => {
  if (currentStage === "record" && !isRecording) {
    startLiveMonitor();
  }
});

micGainSlider.addEventListener("input", () => {
  const pct = parseInt(micGainSlider.value, 10) || 100;
  micGainLabel.textContent = `${pct}%`;
  invoke("rec_set_gain", { gain: pct / 100 }).catch(() => {});
});

micGainSlider.addEventListener("change", async () => {
  const pct = parseInt(micGainSlider.value, 10) || 100;
  try {
    const cfg = await invoke("get_config");
    await invoke("save_config", { cfg: { ...cfg, mic_gain: pct / 100 } });
  } catch {}
});

const LIVE_WAVE_PTS = 240;
let liveWavePeaks = new Array(LIVE_WAVE_PTS).fill(0.01);
let liveWaveRms = new Array(LIVE_WAVE_PTS).fill(0.005);
let liveWaveTimes = new Array(LIVE_WAVE_PTS).fill(null);

function drawAudacityRuler(ctx, rulerW, h, dpr) {
  ctx.fillStyle = "#e2e2e8";
  ctx.fillRect(0, 0, rulerW, h);
  ctx.strokeStyle = "#8e8e98";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(rulerW, 0);
  ctx.lineTo(rulerW, h);
  ctx.stroke();

  const labels = [
    { txt: "1.0", y: 9 * dpr },
    { txt: "0.5", y: h * 0.25 },
    { txt: "0.0", y: h * 0.5 },
    { txt: "-0.5", y: h * 0.75 },
    { txt: "-1.0", y: h - 5 * dpr },
  ];
  ctx.fillStyle = "#1e293b";
  ctx.font = `${Math.round(9.5 * dpr)}px "Plus Jakarta Sans", sans-serif`;
  ctx.textAlign = "right";
  ctx.textBaseline = "middle";
  for (const lb of labels) {
    ctx.fillText(lb.txt, rulerW - 5 * dpr, lb.y);
    ctx.beginPath();
    ctx.moveTo(rulerW - 3 * dpr, lb.y);
    ctx.lineTo(rulerW, lb.y);
    ctx.stroke();
  }
}

function drawLiveWaveform(paused, recording) {
  if (!liveWaveCanvas || !liveWaveCtx) return;
  const dpr = window.devicePixelRatio || 1;
  const cssW = liveWaveCanvas.clientWidth || 900;
  const cssH = liveWaveCanvas.clientHeight || 76;
  const w = Math.floor(cssW * dpr);
  const h = Math.floor(cssH * dpr);
  if (liveWaveCanvas.width !== w || liveWaveCanvas.height !== h) {
    liveWaveCanvas.width = w;
    liveWaveCanvas.height = h;
  }
  const ctx = liveWaveCtx;
  const showTimeRuler = isRecording;
  const topH = showTimeRuler ? Math.round(18 * dpr) : 0;
  const rulerW = Math.round(36 * dpr);
  const trackW = Math.max(10, w - rulerW);
  const mid = h / 2;
  const ampScale = mid - 4 * dpr;
  const trackH = Math.max(20, h - topH);
  const mid = topH + trackH / 2;
  const ampScale = trackH / 2 - 4 * dpr;

  // Audacity silver-grey track background
  ctx.fillStyle = "#c8c8ce";
  ctx.fillRect(0, 0, w, h);
  ctx.fillRect(0, topH, w, trackH);

  if (showTimeRuler) {
    ctx.fillStyle = "#e2e2e8";
    ctx.fillRect(0, 0, w, topH);
    ctx.strokeStyle = "#8e8e98";
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(0, topH);
    ctx.lineTo(w, topH);
    ctx.stroke();

    ctx.fillStyle = "#1e293b";
    ctx.font = `${Math.round(9 * dpr)}px "Plus Jakarta Sans", sans-serif`;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    let lastDrawnSec = -1;
    for (let i = 0; i < LIVE_WAVE_PTS; i += 30) {
      const tVal = liveWaveTimes[i];
      if (typeof tVal !== "number") continue;
      const wholeSec = Math.floor(tVal);
      if (wholeSec === lastDrawnSec) continue;
      lastDrawnSec = wholeSec;
      const x = rulerW + (i / (LIVE_WAVE_PTS - 1)) * trackW;
      ctx.strokeStyle = "#b6b6c0";
      ctx.beginPath();
      ctx.moveTo(x, topH);
      ctx.lineTo(x, h);
      ctx.stroke();
      ctx.strokeStyle = "#64748b";
      ctx.beginPath();
      ctx.moveTo(x, topH - 4 * dpr);
      ctx.lineTo(x, topH);
      ctx.stroke();
      const m = Math.floor(wholeSec / 60);
      const s = wholeSec % 60;
      ctx.fillText(`${m}:${String(s).padStart(2, "0")}`, x, topH * 0.45);
    }
  }

  // Subtle horizontal reference lines
  ctx.strokeStyle = "#b2b2ba";
  ctx.lineWidth = 1;
  for (const frac of [0.25, 0.75]) {
    ctx.beginPath();
    ctx.moveTo(rulerW, h * frac);
    ctx.lineTo(w, h * frac);
    ctx.moveTo(rulerW, topH + trackH * frac);
    ctx.lineTo(w, topH + trackH * frac);
    ctx.stroke();
  }

  // Peak outer continuous envelope (Audacity light royal blue)
  ctx.fillStyle = paused ? "rgba(217, 119, 6, 0.35)" : recording ? "rgba(91, 95, 232, 0.65)" : "rgba(91, 95, 232, 0.55)";
  ctx.beginPath();
  for (let i = 0; i < LIVE_WAVE_PTS; i++) {
    const x = rulerW + (i / (LIVE_WAVE_PTS - 1)) * trackW;
    const y = mid - (liveWavePeaks[i] || 0.01) * ampScale;
    if (i === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  }
  for (let i = LIVE_WAVE_PTS - 1; i >= 0; i--) {
    const x = rulerW + (i / (LIVE_WAVE_PTS - 1)) * trackW;
    const y = mid + (liveWavePeaks[i] || 0.01) * ampScale;
    ctx.lineTo(x, y);
  }
  ctx.closePath();
  ctx.fill();

  // RMS inner continuous envelope (Audacity deep royal blue)
  ctx.fillStyle = paused ? "#d97706" : "#3232c8";
  ctx.beginPath();
  for (let i = 0; i < LIVE_WAVE_PTS; i++) {
    const x = rulerW + (i / (LIVE_WAVE_PTS - 1)) * trackW;
    const y = mid - (liveWaveRms[i] || 0.005) * ampScale;
    if (i === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  }
  for (let i = LIVE_WAVE_PTS - 1; i >= 0; i--) {
    const x = rulerW + (i / (LIVE_WAVE_PTS - 1)) * trackW;
    const y = mid + (liveWaveRms[i] || 0.005) * ampScale;
    ctx.lineTo(x, y);
  }
  ctx.closePath();
  ctx.fill();

  // Center 0.0 line
  ctx.strokeStyle = "#334155";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(rulerW, mid);
  ctx.lineTo(w, mid);
  ctx.stroke();

  drawAudacityRuler(ctx, rulerW, h, dpr);
  if (showTimeRuler) {
    drawAudacityLeftRuler(ctx, rulerW, topH, trackH, dpr);
  } else {
    drawAudacityRuler(ctx, rulerW, h, dpr);
  }
}

listen("rec-level", (e) => {
  const { rms_db, peak, elapsed_s, paused, monitoring } = e.payload;
  const rmsLin = Math.max(0.005, Math.min(1, Math.pow(10, rms_db / 20)));
  const peakLin = Math.max(rmsLin, Math.min(1, typeof peak === "number" ? peak : rmsLin));
  if (!paused) {
    liveWavePeaks.push(peakLin);
    liveWaveRms.push(rmsLin);
    liveWaveTimes.push(monitoring ? null : elapsed_s);
    if (liveWavePeaks.length > LIVE_WAVE_PTS) liveWavePeaks.shift();
    if (liveWaveRms.length > LIVE_WAVE_PTS) liveWaveRms.shift();
    if (liveWaveTimes.length > LIVE_WAVE_PTS) liveWaveTimes.shift();
  }
  drawLiveWaveform(paused, !monitoring && isRecording);
  recDbReadout.textContent = paused ? "Paused" : `${rms_db <= -89 ? "-∞" : rms_db.toFixed(1)} dB`;
  if (!monitoring) {
    recTimer.textContent = formatTimerHMS(elapsed_s);
    recMonitorStatus.textContent = paused ? "Paused" : "Recording";
    recMonitorStatus.classList.toggle("recording", !paused);
    recMonitorStatus.classList.toggle("paused", paused);
  } else if (!isRecording) {
    recMonitorStatus.textContent = "Live Monitor";
    recMonitorStatus.classList.remove("recording", "paused");
  }
});

async function startMicRecording() {
  if (isRecording) return;
  liveWavePeaks.fill(0.01);
  liveWaveRms.fill(0.005);
  liveWaveTimes.fill(null);
  await invoke("rec_start", { deviceName: micSelect.value || null });
  isRecording = true;
  recStartBtn.classList.add("hidden");
  recPauseBtn.classList.remove("hidden");
  recClearBtn.classList.remove("hidden");
  recStopBtn.classList.remove("hidden");
  recPauseBtn.textContent = "Pause";
}

function resetRecButtons() {
  isRecording = false;
  liveWavePeaks.fill(0.01);
  liveWaveRms.fill(0.005);
  liveWaveTimes.fill(null);
  drawLiveWaveform(false, false);
  recTimer.textContent = "00:00:00";
  recStartBtn.classList.remove("hidden");
  recPauseBtn.classList.add("hidden");
  recClearBtn.classList.add("hidden");
  recStopBtn.classList.add("hidden");
}

async function stopMicAndEdit() {
  if (!isRecording) return;
  const wavPath = await invoke("rec_stop");
  originalRecordedWavPath = wavPath;
  resetRecButtons();
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

recClearBtn.addEventListener("click", async () => {
  if (!isRecording) return;
  try {
    const backupPath = await invoke("rec_clear_and_backup", {
      saveDate: saveDateInput.value || null,
    });
    resetRecButtons();
    meetStatusHint.textContent = `Recording cleared. Old take backed up to: ${backupPath}`;
    await startLiveMonitor();
  } catch (err) {
    meetStatusHint.textContent = String(err);
  }
});

recStopBtn.addEventListener("click", async () => {
  try {
    await stopMicAndEdit();
  } catch (err) {
    meetStatusHint.textContent = String(err);
  }
});

let meetLoaded = false;

function isAnyModalOpen() {
  const sOpen = settingsModal && !settingsModal.classList.contains("hidden");
  const dOpen = driveModal && !driveModal.classList.contains("hidden");
  return sOpen || dOpen;
}

function syncMeetBounds() {
  if (!meetLoaded || !meetEmbedHost) return;
  const rect = meetEmbedHost.getBoundingClientRect();
  const top = Math.max(46, rect.top);
  const bottom = Math.min(window.innerHeight - 6, rect.bottom);
  const clippedH = bottom - top;
  const visible = currentStage === "record" && !isAnyModalOpen() && clippedH > 40 && rect.width > 40;
  invoke("set_meet_bounds", {
    x: rect.left,
    y: top,
    width: rect.width,
    height: Math.max(0, clippedH),
    visible,
  }).catch(() => {});
}

if (stages.record) {
  stages.record.addEventListener("scroll", () => syncMeetBounds());
}

async function loadEmbeddedMeet() {
  let url = cfgMeetUrl ? cfgMeetUrl.value.trim() : "";
  if (!url) {
    try {
      const cfg = await invoke("get_config");
      url = (cfg.meet_url || "").trim();
    } catch {}
  }
  if (!url) {
    meetStatusHint.textContent = "Set your Google Meet link in Settings first.";
    return;
  }
  const rect = meetEmbedHost.getBoundingClientRect();
  await invoke("embed_meet", {
    url,
    x: rect.left,
    y: rect.top,
    width: Math.max(200, rect.width),
    height: Math.max(200, rect.height),
  });
  meetLoaded = true;
  meetStatusHint.textContent = "Meet ready — sign in once if prompted.";
  syncMeetBounds();
}

if (typeof ResizeObserver !== "undefined" && meetEmbedHost) {
  new ResizeObserver(() => syncMeetBounds()).observe(meetEmbedHost);
}
window.addEventListener("resize", () => syncMeetBounds());

openMeetBtn.addEventListener("click", async () => {
  try {
    await loadEmbeddedMeet();
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
  try {
    await loadEmbeddedMeet();
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
const edZoomOutBtn = document.getElementById("edZoomOutBtn");
const edZoomFitBtn = document.getElementById("edZoomFitBtn");
const edZoomInBtn = document.getElementById("edZoomInBtn");

let waveSummary = null;
let viewStartS = 0;
let viewEndS = 0;
let viewPeaks = null;
let peakFetchTimer = null;
let selStartS = 0;
let selEndS = 0;
let playheadS = 0;
let isPlaying = false;
let isDraggingWave = false;
let dragAnchorS = 0;
let editorDisplayName = "";

function formatRulerTime(sec, span) {
  const s = Math.max(0, sec);
  if (span < 10) return `${s.toFixed(1)}s`;
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const rem = Math.floor(s % 60);
  if (h > 0) return `${h}:${String(m).padStart(2, "0")}:${String(rem).padStart(2, "0")}`;
  return `${m}:${String(rem).padStart(2, "0")}`;
}

function chooseTickStep(span) {
  const candidates = [0.1, 0.2, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800];
  for (const c of candidates) {
    if (span / c <= 10) return c;
  }
  return 3600;
}

function schedulePeakFetch() {
  if (!waveSummary || waveSummary.duration_s <= 0) return;
  const dur = waveSummary.duration_s;
  const span = viewEndS - viewStartS;
  if (span >= dur * 0.98) {
    viewPeaks = null;
    return;
  }
  clearTimeout(peakFetchTimer);
  peakFetchTimer = setTimeout(async () => {
    try {
      viewPeaks = await invoke("editor_peaks", {
        startS: viewStartS,
        endS: viewEndS,
        buckets: 1600,
      });
      drawWaveform();
    } catch {}
  }, 45);
}

function zoomEditorBy(factor, anchorTimeS) {
  if (!waveSummary || waveSummary.duration_s <= 0) return;
  const dur = waveSummary.duration_s;
  const oldSpan = Math.max(0.2, (viewEndS || dur) - viewStartS);
  const newSpan = Math.max(0.2, Math.min(dur, oldSpan * factor));
  const anchor = typeof anchorTimeS === "number" ? anchorTimeS : (viewStartS + oldSpan / 2);
  const frac = Math.max(0, Math.min(1, (anchor - viewStartS) / oldSpan));
  let nextStart = anchor - frac * newSpan;
  let nextEnd = nextStart + newSpan;
  if (nextStart < 0) {
    nextStart = 0;
    nextEnd = newSpan;
  }
  if (nextEnd > dur) {
    nextEnd = dur;
    nextStart = Math.max(0, dur - newSpan);
  }
  viewStartS = nextStart;
  viewEndS = nextEnd;
  schedulePeakFetch();
  drawWaveform();
}

function drawAudacityLeftRuler(ctx, rulerW, topH, trackH, dpr) {
  ctx.fillStyle = "#e2e2e8";
  ctx.fillRect(0, topH, rulerW, trackH);
  ctx.strokeStyle = "#8e8e98";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(rulerW, topH);
  ctx.lineTo(rulerW, topH + trackH);
  ctx.stroke();

  const labels = [
    { txt: "1.0", y: topH + 8 * dpr },
    { txt: "0.5", y: topH + trackH * 0.25 },
    { txt: "0.0", y: topH + trackH * 0.5 },
    { txt: "-0.5", y: topH + trackH * 0.75 },
    { txt: "-1.0", y: topH + trackH - 6 * dpr },
  ];
  ctx.fillStyle = "#1e293b";
  ctx.font = `${Math.round(9.5 * dpr)}px "Plus Jakarta Sans", sans-serif`;
  ctx.textAlign = "right";
  ctx.textBaseline = "middle";
  for (const lb of labels) {
    ctx.fillText(lb.txt, rulerW - 5 * dpr, lb.y);
    ctx.beginPath();
    ctx.moveTo(rulerW - 3 * dpr, lb.y);
    ctx.lineTo(rulerW, lb.y);
    ctx.stroke();
  }
}

function drawWaveform() {
  const dpr = window.devicePixelRatio || 1;
  const cssW = waveCanvas.clientWidth || 760;
  const cssH = waveCanvas.clientHeight || 170;
  const w = Math.floor(cssW * dpr);
  const h = Math.floor(cssH * dpr);
  if (waveCanvas.width !== w || waveCanvas.height !== h) {
    waveCanvas.width = w;
    waveCanvas.height = h;
  }
  const rulerW = Math.round(36 * dpr);
  const topH = Math.round(22 * dpr);
  const trackW = Math.max(10, w - rulerW);
  const trackH = Math.max(20, h - topH);
  const mid = topH + trackH / 2;
  const ampScale = trackH / 2 - 6 * dpr;

  // Top X-axis time ruler background
  waveCtx.fillStyle = "#e2e2e8";
  waveCtx.fillRect(0, 0, w, topH);
  waveCtx.fillStyle = "#c8c8ce";
  waveCtx.fillRect(rulerW, topH, trackW, trackH);

  // Horizontal 0.5 / -0.5 reference lines
  waveCtx.strokeStyle = "#b2b2ba";
  waveCtx.lineWidth = 1;
  for (const frac of [0.25, 0.75]) {
    waveCtx.beginPath();
    waveCtx.moveTo(rulerW, topH + trackH * frac);
    waveCtx.lineTo(w, topH + trackH * frac);
    waveCtx.stroke();
  }

  if (!waveSummary || !waveSummary.peaks.length || waveSummary.duration_s <= 0) {
    waveCtx.strokeStyle = "#8e8e98";
    waveCtx.beginPath();
    waveCtx.moveTo(0, topH);
    waveCtx.lineTo(w, topH);
    waveCtx.stroke();
    waveCtx.strokeStyle = "#334155";
    waveCtx.beginPath();
    waveCtx.moveTo(rulerW, mid);
    waveCtx.lineTo(w, mid);
    waveCtx.stroke();
    drawAudacityLeftRuler(waveCtx, rulerW, topH, trackH, dpr);
    waveCtx.fillStyle = "#475569";
    waveCtx.font = `${Math.round(13 * dpr)}px "Plus Jakarta Sans", sans-serif`;
    waveCtx.textAlign = "center";
    waveCtx.fillText("No audio loaded yet — record in Stage 1 or click Open File", rulerW + trackW / 2, mid - 12 * dpr);
    return;
  }

  const dur = waveSummary.duration_s;
  const vStart = Math.max(0, Math.min(viewStartS, dur));
  const vEnd = Math.max(vStart + 0.05, Math.min(viewEndS || dur, dur));
  const vSpan = vEnd - vStart;

  // X-axis time ticks + subtle vertical grid
  const tickStep = chooseTickStep(vSpan);
  const firstTick = Math.ceil(vStart / tickStep) * tickStep;
  waveCtx.font = `${Math.round(9.5 * dpr)}px "Plus Jakarta Sans", sans-serif`;
  waveCtx.textAlign = "center";
  waveCtx.textBaseline = "middle";
  for (let t = firstTick; t <= vEnd + 1e-6; t += tickStep) {
    const x = rulerW + ((t - vStart) / vSpan) * trackW;
    waveCtx.strokeStyle = "#b6b6c0";
    waveCtx.beginPath();
    waveCtx.moveTo(x, topH);
    waveCtx.lineTo(x, h);
    waveCtx.stroke();

    waveCtx.strokeStyle = "#64748b";
    waveCtx.beginPath();
    waveCtx.moveTo(x, topH - 5 * dpr);
    waveCtx.lineTo(x, topH);
    waveCtx.stroke();

    waveCtx.fillStyle = "#1e293b";
    waveCtx.fillText(formatRulerTime(t, vSpan), x, topH * 0.45);
  }

  // Ruler bottom border
  waveCtx.strokeStyle = "#8e8e98";
  waveCtx.beginPath();
  waveCtx.moveTo(0, topH);
  waveCtx.lineTo(w, topH);
  waveCtx.stroke();

  // Selection highlight
  const sA = Math.min(selStartS, selEndS);
  const sB = Math.max(selStartS, selEndS);
  if (sB > sA && sB >= vStart && sA <= vEnd) {
    const x1 = rulerW + ((Math.max(sA, vStart) - vStart) / vSpan) * trackW;
    const x2 = rulerW + ((Math.min(sB, vEnd) - vStart) / vSpan) * trackW;
    waveCtx.fillStyle = "rgba(50, 50, 200, 0.16)";
    waveCtx.fillRect(x1, topH, x2 - x1, trackH);
  }

  // Audacity-style continuous filled waveform polygon
  const peaks = viewPeaks || waveSummary.peaks;
  waveCtx.fillStyle = "#3232c8";
  waveCtx.beginPath();
  for (let i = 0; i < peaks.length; i++) {
    const x = rulerW + (i / (peaks.length - 1)) * trackW;
    const [, mx] = peaks[i];
    const yTop = mid - mx * ampScale;
    if (i === 0) waveCtx.moveTo(x, yTop);
    else waveCtx.lineTo(x, yTop);
  }
  for (let i = peaks.length - 1; i >= 0; i--) {
    const x = rulerW + (i / (peaks.length - 1)) * trackW;
    const [mn] = peaks[i];
    const yBot = mid - mn * ampScale;
    waveCtx.lineTo(x, yBot);
  }
  waveCtx.closePath();
  waveCtx.fill();

  // Center 0.0 line
  waveCtx.strokeStyle = "#334155";
  waveCtx.lineWidth = 1;
  waveCtx.beginPath();
  waveCtx.moveTo(rulerW, mid);
  waveCtx.lineTo(w, mid);
  waveCtx.stroke();

  // Playhead
  if (playheadS >= vStart && playheadS <= vEnd) {
    const px = rulerW + ((playheadS - vStart) / vSpan) * trackW;
    waveCtx.strokeStyle = "#059669";
    waveCtx.lineWidth = Math.max(2, 2 * dpr);
    waveCtx.beginPath();
    waveCtx.moveTo(px, topH);
    waveCtx.lineTo(px, h);
    waveCtx.stroke();
  }

  drawAudacityLeftRuler(waveCtx, rulerW, topH, trackH, dpr);

  const zoomPct = Math.round((dur / vSpan) * 100);
  const selText = sB > sA ? ` | Selection: ${formatTime(sA)} - ${formatTime(sB)}` : "";
  const zoomText = zoomPct > 100 ? ` | Zoom: ${zoomPct}%` : "";
  editorTimeInfo.textContent = `Duration: ${formatTime(dur)} | Cursor: ${formatTime(playheadS)}${selText}${zoomText}`;
}

function xToTime(clientX) {
  if (!waveSummary || waveSummary.duration_s <= 0) return 0;
  const dur = waveSummary.duration_s;
  const vStart = Math.max(0, Math.min(viewStartS, dur));
  const vEnd = Math.max(vStart + 0.05, Math.min(viewEndS || dur, dur));
  const rect = waveCanvas.getBoundingClientRect();
  const rulerW = 36;
  const trackW = Math.max(1, rect.width - rulerW);
  const frac = Math.max(0, Math.min(1, (clientX - rect.left - rulerW) / trackW));
  return vStart + frac * (vEnd - vStart);
}

waveCanvas.addEventListener("wheel", (e) => {
  if (!waveSummary || waveSummary.duration_s <= 0) return;
  if (e.ctrlKey) {
    e.preventDefault();
    const factor = e.deltaY < 0 ? 0.72 : 1.38;
    zoomEditorBy(factor, xToTime(e.clientX));
  } else if ((viewEndS - viewStartS) < waveSummary.duration_s * 0.99) {
    e.preventDefault();
    const dur = waveSummary.duration_s;
    const span = viewEndS - viewStartS;
    const delta = (Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY);
    const shift = (delta / 600) * span;
    viewStartS = Math.max(0, Math.min(dur - span, viewStartS + shift));
    viewEndS = viewStartS + span;
    schedulePeakFetch();
    drawWaveform();
  }
}, { passive: false });

if (edZoomInBtn) edZoomInBtn.addEventListener("click", () => zoomEditorBy(0.65, playheadS));
if (edZoomOutBtn) edZoomOutBtn.addEventListener("click", () => zoomEditorBy(1.5, playheadS));
if (edZoomFitBtn) edZoomFitBtn.addEventListener("click", () => {
  if (!waveSummary) return;
  viewStartS = 0;
  viewEndS = waveSummary.duration_s;
  viewPeaks = null;
  drawWaveform();
});

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
    viewStartS = 0;
    viewEndS = waveSummary.duration_s || 0;
    viewPeaks = null;
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

const cfgMeetUrl = document.getElementById("cfgMeetUrl");
const cfgBackupEnabled = document.getElementById("cfgBackupEnabled");
const cfgBackupDirectBtn = document.getElementById("cfgBackupDirectBtn");
const cfgBackupBaseBtn = document.getElementById("cfgBackupBaseBtn");
const cfgBackupDirDisplay = document.getElementById("cfgBackupDirDisplay");
const cfgBackupChooseBtn = document.getElementById("cfgBackupChooseBtn");
let pendingBackupDir = "";
let pendingBackupMode = "base";

function updateBackupModeUI(mode) {
  pendingBackupMode = mode === "direct" ? "direct" : "base";
  cfgBackupDirectBtn.classList.toggle("active", pendingBackupMode === "direct");
  cfgBackupBaseBtn.classList.toggle("active", pendingBackupMode === "base");
}

cfgBackupDirectBtn.addEventListener("click", () => updateBackupModeUI("direct"));
cfgBackupBaseBtn.addEventListener("click", () => updateBackupModeUI("base"));

cfgBackupChooseBtn.addEventListener("click", async () => {
  const dir = await openDialog({ directory: true, multiple: false });
  if (dir) {
    pendingBackupDir = dir;
    cfgBackupDirDisplay.textContent = dir.split(/[\\/]/).pop() || dir;
    cfgBackupDirDisplay.title = dir;
  }
});

function applyConfigToUI(cfg) {
  document.getElementById("cfgFolder").value = cfg.drive_folder_input || "";
  document.getElementById("cfgClientId").value = cfg.oauth_client_id || "";
  document.getElementById("cfgClientSecret").value = cfg.oauth_client_secret || "";
  document.getElementById("cfgMaxMb").value = cfg.max_size_mb ?? 30;
  document.getElementById("cfgLufs").value = cfg.target_lufs ?? -16;
  document.getElementById("cfgPublicLink").checked = cfg.make_public_link ?? true;
  document.getElementById("cfgPublicLink").checked = cfg.make_public_link ?? false;
  cfgMeetUrl.value = cfg.meet_url || "";

  const gainPct = Math.round((cfg.mic_gain ?? 1.0) * 100);
  micGainSlider.value = String(gainPct);
  micGainLabel.textContent = `${gainPct}%`;

  cfgBackupEnabled.checked = cfg.backup_enabled ?? true;
  pendingBackupDir = cfg.backup_dir || "";
  if (pendingBackupDir) {
    cfgBackupDirDisplay.textContent = pendingBackupDir.split(/[\\/]/).pop() || pendingBackupDir;
    cfgBackupDirDisplay.title = pendingBackupDir;
  } else {
    cfgBackupDirDisplay.textContent = "Documents / MessageMaster / Backups";
    cfgBackupDirDisplay.title = "";
  }
  updateBackupModeUI(cfg.backup_save_mode || "base");

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
  const meetVal = cfgMeetUrl.value.trim();
  return {
    drive_folder_input: document.getElementById("cfgFolder").value.trim(),
    drive_folder_name: existing.drive_folder_name || "",
    local_output_dir: existing.local_output_dir || "",
    save_mode: existing.save_mode || "direct",
    drive_save_mode: existing.drive_save_mode || "direct",
    upload_directly: existing.upload_directly ?? false,
    meet_url: meetVal,
    mic_gain: (parseInt(micGainSlider.value, 10) || 100) / 100,
    backup_enabled: cfgBackupEnabled.checked,
    backup_dir: pendingBackupDir,
    backup_save_mode: pendingBackupMode,
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
  if (!overrideDisplay) {
    originalRecordedWavPath = null;
  }
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
      originalBackupSource: originalRecordedWavPath || null,
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
  syncMeetBounds();
});

closeSettingsBtn.addEventListener("click", () => {
  settingsModal.classList.add("hidden");
  syncMeetBounds();
});

saveSettingsBtn.addEventListener("click", async () => {
  const existing = await invoke("get_config");
  const updated = readConfigFromUI(existing);
  if (updated.drive_folder_input !== existing.drive_folder_input) {
    updated.drive_folder_name = updated.drive_folder_input ? updated.drive_folder_input : "My Drive (Root)";
  }
  await invoke("save_config", { cfg: updated });
  applyConfigToUI(updated);
  settingsModal.classList.add("hidden");
  syncMeetBounds();
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
