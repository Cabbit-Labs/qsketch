// Browser player for a qSketch rig export (rig.json + parts/*.png). The
// posing lives in rig-runtime.js; this file only loads images, draws and
// wires up the controls.
"use strict";

(function () {
  const $ = (id) => document.getElementById(id);
  const view = $("view");
  const ctx = view.getContext("2d");
  const status = (text, error) => {
    $("status").textContent = text;
    $("status").className = error ? "error" : "";
  };

  const state = {
    rig: null,
    images: new Map(), // part image path -> HTMLImageElement
    anim: null, // animation name, or null for the setup pose
    time: 0, // seconds, as played (not wrapped)
    playing: true,
    speed: 1,
    zoom: 2,
    bones: false,
    pixel: false,
    last: 0,
  };

  function loadImage(url) {
    return new Promise((resolve, reject) => {
      const img = new Image();
      img.onload = () => resolve(img);
      img.onerror = () => reject(new Error("could not load " + url));
      img.src = url;
    });
  }

  /** Load a rig from a JSON object, resolving part images with `urlFor(path)`. */
  async function open(json, urlFor, name) {
    const rig = QRig.load(json);
    const images = new Map();
    await Promise.all(
      rig.parts.map(async (p) => {
        images.set(p.image, await loadImage(urlFor(p.image)));
      })
    );
    state.rig = rig;
    state.images = images;
    const select = $("anim");
    select.replaceChildren(new Option("Setup pose", ""));
    for (const n of Object.keys(rig.animations)) select.add(new Option(n, n));
    const first = Object.keys(rig.animations)[0] ?? null;
    select.value = first ?? "";
    setAnimation(first);
    status(`${name}: ${rig.bones.length} bones, ${rig.parts.length} parts, ${Object.keys(rig.animations).length} animations`);
    sizeCanvas();
  }

  function setAnimation(name) {
    state.anim = name;
    state.time = 0;
    state.playing = name != null;
    const an = name != null ? state.rig.animations[name] : null;
    $("time").max = an ? an.duration : 1;
    $("time").disabled = !an;
    $("play").disabled = !an;
    $("play").textContent = state.playing ? "Pause" : "Play";
  }

  function sizeCanvas() {
    if (!state.rig) return;
    const dpr = window.devicePixelRatio || 1;
    const { width, height } = state.rig.canvas;
    view.width = Math.round(width * state.zoom * dpr);
    view.height = Math.round(height * state.zoom * dpr);
    view.style.width = width * state.zoom + "px";
  }

  function drawBackdrop(w, h) {
    const s = 12;
    for (let y = 0; y < h; y += s) {
      for (let x = 0; x < w; x += s) {
        ctx.fillStyle = ((x / s + y / s) & 1) === 0 ? "#2c313b" : "#262a33";
        ctx.fillRect(x, y, s, s);
      }
    }
  }

  function drawBones(rig, world) {
    ctx.lineWidth = 2 / state.zoom;
    for (const c of rig.ik) {
      const tip = QRig.apply(world[c.child], rig.bones[c.child].length, 0);
      const to = QRig.apply(world[c.target], 0, 0);
      ctx.strokeStyle = "#6edc6e";
      ctx.setLineDash([5 / state.zoom, 4 / state.zoom]);
      ctx.beginPath();
      ctx.moveTo(tip.x, tip.y);
      ctx.lineTo(to.x, to.y);
      ctx.stroke();
      ctx.setLineDash([]);
    }
    rig.bones.forEach((b, i) => {
      const head = QRig.apply(world[i], 0, 0);
      const tip = QRig.apply(world[i], b.length, 0);
      ctx.strokeStyle = "#5ac8ff";
      ctx.fillStyle = "#5ac8ff";
      ctx.beginPath();
      ctx.moveTo(head.x, head.y);
      ctx.lineTo(tip.x, tip.y);
      ctx.stroke();
      ctx.beginPath();
      ctx.arc(head.x, head.y, 3 / state.zoom, 0, Math.PI * 2);
      ctx.fill();
    });
  }

  function draw() {
    const rig = state.rig;
    if (!rig) return;
    const scale = state.zoom * (window.devicePixelRatio || 1);
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.globalAlpha = 1;
    ctx.globalCompositeOperation = "source-over";
    drawBackdrop(view.width, view.height);
    ctx.imageSmoothingEnabled = !state.pixel;
    const world = QRig.pose(rig, state.anim, state.time);
    for (const part of rig.parts) {
      const m = QRig.partMatrix(rig, part, world);
      ctx.setTransform(scale, 0, 0, scale, 0, 0);
      ctx.transform(m.a, m.b, m.c, m.d, m.e, m.f);
      ctx.globalAlpha = part.opacity;
      ctx.globalCompositeOperation = QRig.canvasBlend(part.blend);
      ctx.drawImage(state.images.get(part.image), 0, 0);
    }
    ctx.globalAlpha = 1;
    ctx.globalCompositeOperation = "source-over";
    if (state.bones) {
      ctx.setTransform(scale, 0, 0, scale, 0, 0);
      drawBones(rig, world);
    }
  }

  function updateReadout() {
    const an = state.anim != null ? state.rig.animations[state.anim] : null;
    if (!an) {
      $("readout").textContent = "setup pose";
      return;
    }
    const shown = an.loop ? state.time % an.duration : Math.min(state.time, an.duration);
    $("time").value = shown;
    $("readout").textContent = `${shown.toFixed(2)} / ${an.duration.toFixed(2)} s`;
  }

  function frame(now) {
    const dt = Math.min((now - state.last) / 1000, 0.1);
    state.last = now;
    const an = state.rig && state.anim != null ? state.rig.animations[state.anim] : null;
    if (an && state.playing) {
      state.time += dt * state.speed;
      if (!an.loop && state.time >= an.duration) {
        state.time = an.duration;
        state.playing = false;
        $("play").textContent = "Replay";
      }
    }
    if (state.rig) {
      draw();
      updateReadout();
    }
    requestAnimationFrame(frame);
  }

  // --- controls ----------------------------------------------------------------
  $("anim").addEventListener("change", (e) => setAnimation(e.target.value || null));
  $("play").addEventListener("click", () => {
    const an = state.rig.animations[state.anim];
    if (!an) return;
    if (!state.playing && !an.loop && state.time >= an.duration) state.time = 0;
    state.playing = !state.playing;
    $("play").textContent = state.playing ? "Pause" : "Play";
  });
  $("time").addEventListener("input", (e) => {
    state.playing = false;
    $("play").textContent = "Play";
    state.time = parseFloat(e.target.value);
  });
  $("speed").addEventListener("change", (e) => (state.speed = parseFloat(e.target.value)));
  $("zoom").addEventListener("change", (e) => {
    state.zoom = parseFloat(e.target.value);
    sizeCanvas();
  });
  $("bones").addEventListener("change", (e) => (state.bones = e.target.checked));
  $("pixel").addEventListener("change", (e) => (state.pixel = e.target.checked));

  // An exported folder picked from disk: rig.json plus parts/*.png.
  $("folder").addEventListener("change", async (e) => {
    const files = [...e.target.files];
    // webkitRelativePath is "<folder>/rig.json": drop the folder name.
    const rel = (f) => f.webkitRelativePath.split("/").slice(1).join("/");
    const byPath = new Map(files.map((f) => [rel(f), f]));
    const json = byPath.get("rig.json");
    if (!json) return status("That folder has no rig.json.", true);
    const urls = new Map();
    try {
      await open(JSON.parse(await json.text()), (p) => {
        const f = byPath.get(p);
        if (!f) throw new Error("missing " + p);
        if (!urls.has(p)) urls.set(p, URL.createObjectURL(f));
        return urls.get(p);
      }, files[0].webkitRelativePath.split("/")[0]);
    } catch (err) {
      status(err.message, true);
    }
  });

  // --- start ---------------------------------------------------------------------
  const rigUrl = new URLSearchParams(location.search).get("rig") || "sample/rig.json";
  fetch(rigUrl)
    .then((r) => {
      if (!r.ok) throw new Error(`${rigUrl}: ${r.status}`);
      return r.json();
    })
    .then((json) => open(json, (p) => new URL(p, new URL(rigUrl, location.href)).href, rigUrl))
    .catch((err) => status("Could not load the rig: " + err.message, true));
  requestAnimationFrame(frame);

  // For poking at it from the console or a test.
  window.rigPlayer = { state, setAnimation, draw };
})();
