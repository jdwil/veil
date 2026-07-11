// Generated SPA entry for veilRuntimeUI (CAP-005) — same-origin /api
const NAV = [
      { href: ""/"", label: "Dashboard" },
      { href: ""/projects"", label: "Projects" },
      { href: ""/deploy"", label: "Deploy" },
      { href: ""/registry"", label: "Registry" },
      { href: ""/bus"", label: "Bus" },
      { href: ""/agents"", label: "Agents" },
      { href: ""/config"", label: "Config" }
];

async function api(path, opts) {
  const r = await fetch(path, {
    headers: { "Content-Type": "application/json", ...(opts?.headers || {}) },
    ...opts,
  });
  if (!r.ok) throw new Error(await r.text());
  return r.json();
}

function el(tag, attrs = {}, ...kids) {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "className") n.className = v;
    else if (k.startsWith("on") && typeof v === "function") n.addEventListener(k.slice(2).toLowerCase(), v);
    else if (v != null) n.setAttribute(k, v);
  }
  for (const c of kids.flat()) {
    if (c == null) continue;
    n.appendChild(typeof c === "string" ? document.createTextNode(c) : c);
  }
  return n;
}

function shell(main) {
  const root = document.getElementById("app");
  root.replaceChildren(
    el("aside", { className: "sidebar" },
      el("div", { className: "logo" }, el("span", {}, "◆"), " veil-runtime"),
      el("nav", {}, ...NAV.map(i => el("a", { href: i.href, className: location.pathname === i.href ? "active" : "" }, i.label)))
    ),
    el("main", {}, main),
  );
}

async function viewDashboard() {
  let projects = [];
  try {
    const data = await api("/api/projects");
    projects = data.projects || data.repos || [];
  } catch (e) {
    shell(el("div", {}, el("h1", {}, "Dashboard"), el("p", { className: "err" }, String(e))));
    return;
  }
  shell(el("div", {},
    el("h1", {}, "Dashboard"),
    el("p", { className: "sub" }, "Generated shell · live multi-project API"),
    el("div", { className: "stats" },
      el("div", { className: "stat" }, el("div", { className: "v" }, String(projects.length)), el("div", { className: "l" }, "Projects")),
    ),
    el("h2", {}, "Projects"),
    ...projects.map(p => {
      const name = p.name || p.id || "?";
      return el("a", { className: "card", href: `/projects/${encodeURIComponent(name)}/ide` },
        el("div", { className: "name" }, name),
        el("div", { className: "meta" }, p.path || p.default_branch || "open IDE"),
      );
    }),
  ));
}

async function viewProjects() {
  await viewDashboard();
}

async function viewConfig() {
  let cfg = {};
  try { cfg = await api("/api/config"); } catch (e) {
    shell(el("div", {}, el("h1", {}, "Config"), el("p", { className: "err" }, String(e))));
    return;
  }
  const input = el("input", { value: cfg.projects_dir || "", id: "pd" });
  const status = el("p", { className: "sub" }, "");
  const save = el("button", {
    type: "button",
    onClick: async () => {
      try {
        const body = { projects_dir: input.value };
        const r = await api("/api/config", { method: "PATCH", body: JSON.stringify(body) });
        status.textContent = r.ok === false ? (r.error || "failed") : "Saved.";
      } catch (e) { status.textContent = String(e); }
    },
  }, "Save projects_dir");
  shell(el("div", {},
    el("h1", {}, "Config"),
    el("p", { className: "sub" }, cfg.config_path || ""),
    el("label", {}, "projects_dir"),
    el("div", { className: "row" }, input, save),
    status,
  ));
}

function route() {
  const p = location.pathname;
  if (p.startsWith("/config")) return viewConfig();
  if (p.startsWith("/projects")) return viewProjects();
  return viewDashboard();
}

route();
window.addEventListener("popstate", route);
