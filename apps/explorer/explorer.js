/** Semillas de bootstrap. Dominio público primero (usuarios sin el repo). */
/** Única puerta pública: el dominio. Los nodos se ligan aquí. */
const SEEDS = [
  "https://rpc.reeskova-chain.xyz/rpc",
  "http://rpc.reeskova-chain.xyz/rpc",
];

if (location.protocol !== "file:") {
  SEEDS.unshift(new URL("/rpc", location.origin).href);
}

const RECENT = 16;
const view = document.getElementById("view");
const stats = document.getElementById("stats");
const network = document.getElementById("network");
const apiNote = document.getElementById("api-note");

let drawnKey = "";
let activeApi = SEEDS[0];
let knownEndpoints = SEEDS.slice();

apiNote.textContent = "discovery · " + knownEndpoints.join(" · ");

document.getElementById("search").addEventListener("submit", (event) => {
  event.preventDefault();
  const query = document.getElementById("query").value.trim();
  if (!query) {
    location.hash = "#/";
    return;
  }
  if (/^\d+$/.test(query)) {
    location.hash = "#/block/" + query;
    return;
  }
  location.hash = "#/lookup/" + query.toLowerCase();
});

window.addEventListener("hashchange", () => {
  drawnKey = "";
  draw();
});

function normalizeRpcUrl(url) {
  const trimmed = String(url || "").trim().replace(/\/+$/, "");
  if (!trimmed) return null;
  if (!/^https?:\/\//i.test(trimmed)) return null;
  return /\/rpc$/i.test(trimmed) ? trimmed : trimmed + "/rpc";
}

function mergeEndpoints(list) {
  const seen = new Set();
  const out = [];
  for (const raw of list) {
    const url = normalizeRpcUrl(raw);
    if (!url || seen.has(url)) continue;
    seen.add(url);
    out.push(url);
  }
  return out;
}

async function rpcOnce(api, method, params) {
  const response = await fetch(api, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params: params || {} }),
  });
  if (!response.ok) {
    throw new Error("la API no respondió");
  }
  const body = await response.json();
  if (body.error) {
    const error = new Error(body.error.message || "error");
    error.code = body.error.code;
    throw error;
  }
  return body.result;
}

async function refreshDirectory(api) {
  try {
    const listed = await rpcOnce(api, "rsc_getRpcEndpoints", {});
    const urls = Array.isArray(listed)
      ? listed.map((entry) => entry && entry.url).filter(Boolean)
      : [];
    knownEndpoints = mergeEndpoints([api].concat(urls).concat(SEEDS));
    apiNote.textContent = "activo " + api + " · seeds " + knownEndpoints.length;
  } catch (_) {
    /* keep previous directory */
  }
}

async function rpc(method, params) {
  const order = mergeEndpoints([activeApi].concat(knownEndpoints).concat(SEEDS));
  let lastError = new Error("ningún RPC respondió");
  for (const api of order) {
    try {
      const result = await rpcOnce(api, method, params);
      if (api !== activeApi) {
        activeApi = api;
        await refreshDirectory(api);
      }
      return result;
    } catch (error) {
      lastError = error;
    }
  }
  throw lastError;
}

function esc(value) {
  return String(value)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function short(value) {
  const text = String(value || "");
  if (text.length <= 22) return text;
  return text.slice(0, 10) + "…" + text.slice(-8);
}

function when(seconds) {
  const date = new Date(Number(seconds) * 1000);
  if (Number.isNaN(date.getTime())) return "—";
  return date.toLocaleString("es");
}

function route() {
  const hash = location.hash || "#/";
  const parts = hash.replace(/^#\/?/, "").split("/").filter(Boolean);
  return { name: parts[0] || "home", id: decodeURIComponent(parts.slice(1).join("/")) };
}

async function draw() {
  let status;
  let info;
  try {
    [status, info] = await Promise.all([rpc("rsc_getStatus"), rpc("rsc_getNodeInfo")]);
    await refreshDirectory(activeApi);
  } catch (error) {
    stats.innerHTML = "";
    view.innerHTML = `<p class="error">${esc(error.message)}</p>`;
    return;
  }
  network.textContent = info.network_id + " · cadena " + info.chain_id + " · " + info.sync_status;
  const current = route();
  const pageKey = current.name === "home"
    ? "home:" + status.latest_height
    : current.name + ":" + current.id;
  stats.innerHTML = [
    stat("Altura", status.latest_height, "#/block/" + status.latest_height),
    stat("Finalizado", status.finalized_height, "#/block/" + status.finalized_height),
    stat("Época", status.epoch),
    stat("Sincronizando", status.syncing ? "sí" : "no"),
  ].join("");
  if (pageKey === drawnKey) return;
  try {
    if (current.name === "home") await drawHome(status);
    else if (current.name === "block") await drawBlock(current.id);
    else if (current.name === "tx") await drawTx(current.id);
    else if (current.name === "account") await drawAccount(current.id);
    else if (current.name === "lookup") await drawLookup(current.id);
    else view.innerHTML = `<p class="note">Esa página no existe.</p>`;
    const now = route();
    const nowKey = now.name === "home" ? "home:" + status.latest_height : now.name + ":" + now.id;
    if (nowKey === pageKey) drawnKey = pageKey;
  } catch (error) {
    view.innerHTML = `<p class="error">${esc(error.message)}</p>`;
  }
}

function stat(label, value, href) {
  const shown = href
    ? `<a href="${esc(href)}">${esc(value)}</a>`
    : esc(value);
  return `<article class="stat"><span>${esc(label)}</span><strong>${shown}</strong></article>`;
}

async function drawHome(status) {
  const tip = Number(status.latest_height);
  const heights = [];
  for (let height = tip; height >= 0 && heights.length < RECENT; height -= 1) {
    heights.push(height);
  }
  const blocks = [];
  for (const height of heights) {
    try {
      blocks.push(await rpc("rsc_getBlockByHeight", { height }));
    } catch (_) {
      /* skip missing */
    }
  }
  view.innerHTML = `
    <section>
      <h2>Bloques recientes</h2>
      <div class="list">
        ${blocks.map((block) => `
          <a class="row" href="#/block/${esc(block.height)}">
            <strong>#${esc(block.height)}</strong>
            <span>${esc(short(block.hash))}</span>
            <span>${esc(when(block.timestamp))}</span>
            <span>${esc((block.transactions || []).length)} tx</span>
          </a>
        `).join("") || `<p class="note">Sin bloques.</p>`}
      </div>
    </section>
  `;
}

async function drawBlock(id) {
  const block = /^\d+$/.test(id)
    ? await rpc("rsc_getBlockByHeight", { height: Number(id) })
    : await rpc("rsc_getBlock", { block_hash: id });
  view.innerHTML = `
    <section>
      <h2>Bloque #${esc(block.height)}</h2>
      <dl class="meta">
        <div><dt>Hash</dt><dd>${esc(block.hash)}</dd></div>
        <div><dt>Padre</dt><dd><a href="#/block/${esc(block.parent_hash)}">${esc(short(block.parent_hash))}</a></dd></div>
        <div><dt>Timestamp</dt><dd>${esc(when(block.timestamp))}</dd></div>
        <div><dt>Tx root</dt><dd>${esc(block.transaction_root)}</dd></div>
      </dl>
      <h3>Transacciones</h3>
      <div class="list">
        ${(block.transactions || []).map((hash) => `
          <a class="row" href="#/tx/${esc(hash)}"><span>${esc(hash)}</span></a>
        `).join("") || `<p class="note">Sin transacciones.</p>`}
      </div>
    </section>
  `;
}

async function drawTx(id) {
  const tx = await rpc("rsc_getTransaction", { transaction_hash: id });
  view.innerHTML = `
    <section>
      <h2>Transacción</h2>
      <dl class="meta">
        <div><dt>Hash</dt><dd>${esc(tx.hash)}</dd></div>
        <div><dt>Estado</dt><dd>${esc(tx.status)}</dd></div>
        <div><dt>Bloque</dt><dd>${tx.block_height == null ? "—" : `<a href="#/block/${esc(tx.block_height)}">#${esc(tx.block_height)}</a>`}</dd></div>
        <div><dt>Índice</dt><dd>${esc(tx.index == null ? "—" : tx.index)}</dd></div>
      </dl>
      <pre class="raw">${esc(tx.raw || "")}</pre>
    </section>
  `;
}

async function drawAccount(id) {
  const account = await rpc("rsc_getAccount", { account: id });
  view.innerHTML = `
    <section>
      <h2>Cuenta</h2>
      <dl class="meta">
        <div><dt>ID</dt><dd>${esc(account.account)}</dd></div>
        <div><dt>Balance</dt><dd>${esc(account.balance)}</dd></div>
        <div><dt>Nonce</dt><dd>${esc(account.nonce)}</dd></div>
      </dl>
    </section>
  `;
}

async function drawLookup(id) {
  try {
    await drawBlock(id);
    return;
  } catch (_) {}
  try {
    await drawTx(id);
    return;
  } catch (_) {}
  try {
    await drawAccount(id);
    return;
  } catch (_) {}
  view.innerHTML = `<p class="error">No se encontró ${esc(id)}</p>`;
}

draw();
setInterval(draw, 8000);
