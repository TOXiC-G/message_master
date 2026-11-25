const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;
const { open: openDialog } = window.__TAURI__.dialog;
const { open: openShell } = window.__TAURI__.shell;

const appWindow = getCurrentWindow();

let selectedPath = null;
let currentWhatsappUrl = "";
let updateReleaseUrl = "";
let driveNavStack = [{ id: "root", name: "My Drive" }];

// Elements
const dropZone = document.getElementById("dropZone");
const dropTitle = document.getElementById("dropTitle");
const dropSub = document.getElementById("dropSub");
const customNameInput = document.getElementById("customNameInput");
const destFolderName = document.getElementById("destFolderName");
const browseDriveBtn = document.getElementById("browseDriveBtn");
const startBtn = document.getElementById("startBtn");
const uploadToggle = document.getElementById("uploadToggle");
const driveStatusDot = document.getElementById("driveStatusDot");
const progressCard = document.getElementById("progressCard");
const progressLabel = document.getElementById("progressLabel");
const progressStep = document.getElementById("progressStep");
const progressFill = document.getElementById("progressFill");
const errorBanner = document.getElementById("errorBanner");
const resultCard = document.getElementById("resultCard");
const shareMessage = document.getElementById("shareMessage");
const copyBtn = document.getElementById("copyBtn");
const copyBtnText = document.getElementById("copyBtnText");
const whatsappBtn = document.getElementById("whatsappBtn");
const driveWarn = document.getElementById("driveWarn");

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

// Window Controls
document.getElementById("minBtn").addEventListener("click", () => appWindow.minimize());
document.getElementById("maxBtn").addEventListener("click", () => appWindow.toggleMaximize());
document.getElementById("closeBtn").addEventListener("click", () => appWindow.close());

async function init() {
  try {
    const ver = await invoke("get_app_version");
    document.getElementById("appVersion").textContent = `v${ver}`;
    const cfg = await invoke("get_config");
    applyConfigToUI(cfg);
  } catch (e) {
    console.error(e);
  }
}

function applyConfigToUI(cfg) {
  document.getElementById("cfgFolder").value = cfg.drive_folder_input || "";
  document.getElementById("cfgClientId").value = cfg.oauth_client_id || "";
  document.getElementById("cfgClientSecret").value = cfg.oauth_client_secret || "";
  document.getElementById("cfgMaxMb").value = cfg.max_size_mb ?? 30;
  document.getElementById("cfgLufs").value = cfg.target_lufs ?? -16;
  document.getElementById("cfgPublicLink").checked = cfg.make_public_link ?? true;

  if (cfg.drive_folder_name) {
    destFolderName.textContent = cfg.drive_folder_name;
  } else if (cfg.drive_folder_input) {
    destFolderName.textContent = cfg.drive_folder_input;
  } else {
    destFolderName.textContent = "My Drive (Root)";
  }

  const connected = Boolean(cfg.refresh_token);
  driveStatusDot.classList.toggle("connected", connected);
  oauthStatus.textContent = connected ? "Connected to Google Drive" : "Not connected";
  oauthStatus.classList.toggle("connected", connected);
}

function readConfigFromUI(existing) {
  return {
    drive_folder_input: document.getElementById("cfgFolder").value.trim(),
    drive_folder_name: existing.drive_folder_name || "",
    oauth_client_id: document.getElementById("cfgClientId").value.trim(),
    oauth_client_secret: document.getElementById("cfgClientSecret").value.trim(),
    refresh_token: existing.refresh_token,
    max_size_mb: parseFloat(document.getElementById("cfgMaxMb").value) || 30.0,
    target_lufs: parseFloat(document.getElementById("cfgLufs").value) || -16.0,
    make_public_link: document.getElementById("cfgPublicLink").checked,
  };
}

function cleanBaseName(fileName) {
  return fileName
    .replace(/\.(aup3|mp3|wav|mpeg|m4a|flac|ogg)$/i, "")
    .replace(/\.mp3$/i, "")
    .trim();
}

function setSelectedFile(path) {
  selectedPath = path;
  const fileName = path.split(/[\\/]/).pop();
  dropTitle.textContent = fileName;
  dropSub.textContent = path;
  customNameInput.value = cleanBaseName(fileName);
  startBtn.disabled = false;
  errorBanner.classList.add("hidden");
}

async function pickFile() {
  const file = await openDialog({
    multiple: false,
    filters: [
      {
        name: "Audacity & Audio Files",
        extensions: ["aup3", "mp3", "wav", "mpeg", "m4a", "flac", "ogg"],
      },
    ],
  });
  if (file) {
    setSelectedFile(file);
  }
}

dropZone.addEventListener("click", pickFile);
dropZone.addEventListener("keydown", (e) => {
  if (e.key === "Enter" || e.key === " ") {
    e.preventDefault();
    pickFile();
  }
});

// Tauri native drag-and-drop
listen("tauri://drag-enter", () => dropZone.classList.add("dragover"));
listen("tauri://drag-leave", () => dropZone.classList.remove("dragover"));
listen("tauri://drag-drop", (event) => {
  dropZone.classList.remove("dragover");
  const paths = event.payload?.paths || [];
  if (paths.length > 0) {
    setSelectedFile(paths[0]);
  }
});

// Progress listener
listen("mastering-progress", (event) => {
  const { step, label } = event.payload;
  progressCard.classList.remove("hidden");
  progressLabel.textContent = label;
  progressStep.textContent = `Step ${step} / 6`;
  const pct = Math.min(100, Math.round((step / 6) * 100));
  progressFill.style.width = `${pct}%`;
});

// Background update listener
listen("update-available", (event) => {
  const info = event.payload;
  const badge = document.getElementById("updateBadge");
  document.getElementById("updateText").textContent = `Update available (v${info.latest_version})`;
  updateReleaseUrl = info.release_url;
  badge.classList.remove("hidden");
});

document.getElementById("updateBadge").addEventListener("click", () => {
  if (updateReleaseUrl) openShell(updateReleaseUrl);
});

startBtn.addEventListener("click", async () => {
  if (!selectedPath) return;
  startBtn.disabled = true;
  errorBanner.classList.add("hidden");
  resultCard.classList.add("hidden");
  progressCard.classList.remove("hidden");
  progressFill.style.width = "8%";
  progressStep.textContent = "Step 1 / 6";
  progressLabel.textContent = "Starting mastering pipeline...";

  try {
    const res = await invoke("process_and_upload", {
      inputPath: selectedPath,
      customName: customNameInput.value.trim() || null,
      uploadToDrive: uploadToggle.checked,
    });

    progressCard.classList.add("hidden");
    resultCard.classList.remove("hidden");

    document.getElementById("mClicks").textContent = res.clicks_repaired;
    document.getElementById("mSilences").textContent = `${res.seconds_trimmed.toFixed(1)}s`;
    document.getElementById("mSize").textContent = `${res.output_size_mb.toFixed(2)} MB`;
    document.getElementById("mBitrate").textContent = `${res.bitrate_kbps} kbps MP3`;
    document.getElementById("mLufs").textContent = `${res.final_lufs.toFixed(1)} LUFS`;

    shareMessage.value = res.share_message;
    currentWhatsappUrl = res.whatsapp_url;

    if (res.drive_error) {
      driveWarn.textContent = `Drive upload skipped: ${res.drive_error}`;
      driveWarn.classList.remove("hidden");
    } else {
      driveWarn.classList.add("hidden");
    }
  } catch (err) {
    progressCard.classList.add("hidden");
    errorBanner.textContent = String(err);
    errorBanner.classList.remove("hidden");
  } finally {
    startBtn.disabled = false;
  }
});

copyBtn.addEventListener("click", async () => {
  await navigator.clipboard.writeText(shareMessage.value);
  copyBtnText.textContent = "Copied!";
  setTimeout(() => {
    copyBtnText.textContent = "Copy Message";
  }, 1800);
});

whatsappBtn.addEventListener("click", () => {
  if (currentWhatsappUrl) {
    openShell(currentWhatsappUrl);
  }
});

// --- Google Drive Folder Browser ---
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
      const btn = document.createElement("button");
      btn.className = "folder-item";
      btn.type = "button";
      btn.innerHTML = `
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="#7c3aed" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
        </svg>
        <span></span>
      `;
      btn.querySelector("span").textContent = f.name;
      btn.addEventListener("click", () => {
        driveNavStack.push({ id: f.id, name: f.name });
        renderDriveFolder(f.id);
      });
      driveFolderList.appendChild(btn);
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
  } catch (err) {
    driveBrowserStatus.textContent = String(err);
  }
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
  updateCheckStatus.textContent = "Checking...";
  updateCheckStatus.classList.remove("connected");
  try {
    const info = await invoke("check_updates_now");
    if (info) {
      updateCheckStatus.textContent = `Update available: v${info.latest_version}`;
      updateCheckStatus.classList.add("connected");
    } else {
      const ver = await invoke("get_app_version");
      updateCheckStatus.textContent = `You're up to date (v${ver})`;
    }
  } catch (err) {
    updateCheckStatus.textContent = "Could not check for updates.";
  }
});

init();
