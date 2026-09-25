// Interface SilenceLess — utilise l'API globale Tauri (window.__TAURI__).
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);

function currentSettings() {
  return {
    thresholdDb: parseFloat($("threshold").value) || -30,
    minSilenceMs: parseInt($("min-silence").value, 10) || 500,
    paddingMs: parseInt($("padding").value, 10) || 100,
    seekStepMs: 10,
    bitrateKbps: parseInt($("bitrate").value, 10) || 192,
    trimInternal: $("trim-internal").checked,
  };
}

function resetResults() {
  $("results").hidden = false;
  $("list").innerHTML = "";
  $("bar").style.width = "0%";
  $("status").textContent = "Traitement…";
}

function appendLine(ok, text) {
  const li = document.createElement("li");
  li.className = ok ? "ok" : "err";
  li.textContent = (ok ? "✔ " : "✖ ") + text;
  $("list").appendChild(li);
}

async function start(folderOrFiles) {
  const settings = currentSettings();
  const out = $("out").value.trim() || null;
  resetResults();
  try {
    if (Array.isArray(folderOrFiles)) {
      await invoke("process_files", { paths: folderOrFiles, settings, out });
    } else {
      await invoke("process_folder", { folder: folderOrFiles, settings, out });
    }
  } catch (err) {
    $("status").textContent = "Erreur : " + err;
  }
}

$("start").addEventListener("click", () => {
  const folder = $("folder").value.trim();
  if (!folder) {
    $("status").textContent = "Indiquez un dossier (ou glissez-le dans la zone).";
    $("results").hidden = false;
    return;
  }
  start(folder);
});

$("pick").addEventListener("click", async () => {
  try {
    const picked = await invoke("plugin:dialog|open", {
      options: { directory: true, multiple: false },
    });
    if (picked) {
      $("folder").value = picked;
    }
  } catch (err) {
    console.error(err);
  }
});

listen("silence-progress", (event) => {
  const p = event.payload;
  appendLine(p.ok, p.name + (p.ok ? " — " + p.message : " — " + p.message));
  if (p.total > 0) {
    $("bar").style.width = Math.round((100 * p.done) / p.total) + "%";
    $("status").textContent = p.done + " / " + p.total;
  }
});

listen("silence-done", (event) => {
  const d = event.payload;
  const msg = "Terminé — " + d.total + " fichier(s), " + d.failures + " erreur(s).";
  $("status").textContent = msg;
  $("bar").style.width = "100%";
});

listen("drop-active", (event) => {
  $("drop-zone").classList.toggle("dragging", event.payload === true);
});

listen("drop-paths", (event) => {
  const paths = event.payload || [];
  const wavs = paths.filter((p) => p.toLowerCase().endsWith(".wav"));
  if (wavs.length > 0) {
    $("folder").value = wavs[0].replace(/[\\/][^\\/]*$/, "");
    start(wavs);
  } else if (paths.length === 1) {
    $("folder").value = paths[0];
    start(paths[0]);
  }
});
