// Generated SPA entry for veilRuntimeUI (CAP-005) — same-origin /api
const NAV = [
  { href: "/", label: "Dashboard" },
  { href: "/projects", label: "Projects" },
  { href: "/deploy", label: "Deploy" },
  { href: "/registry", label: "Registry" },
  { href: "/bus", label: "Bus" },
  { href: "/agents", label: "Agents" },
  { href: "/config", label: "Config" },
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
    else if (k.startsWith("on") && typeof v === "function")
      n.addEventListener(k.slice(2).toLowerCase(), v);
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
    el(
      "aside",
      { className: "sidebar" },
      el("div", { className: "logo" }, el("span", {}, "◆"), " veil-runtime"),
      el(
        "nav",
        {},
        ...NAV.map((i) =>
          el(
            "a",
            {
              href: i.href,
              className: location.pathname === i.href ? "active" : "",
            },
            i.label,
          ),
        ),
      ),
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
    shell(
      el(
        "div",
        {},
        el("h1", {}, "Dashboard"),
        el("p", { className: "err" }, String(e)),
      ),
    );
    return;
  }
  shell(
    el(
      "div",
      {},
      el("h1", {}, "Dashboard"),
      el("p", { className: "sub" }, "Generated shell · live multi-project API"),
      el(
        "div",
        { className: "stats" },
        el(
          "div",
          { className: "stat" },
          el("div", { className: "v" }, String(projects.length)),
          el("div", { className: "l" }, "Projects"),
        ),
      ),
      el("h2", {}, "Projects"),
      ...projects.map((p) => {
        const name = p.name || p.id || "?";
        return el(
          "a",
          {
            className: "card",
            href: `/viewer/?project=${encodeURIComponent(name)}`,
          },
          el("div", { className: "name" }, name),
          el(
            "div",
            { className: "meta" },
            p.path || p.default_branch || "open IDE",
          ),
        );
      }),
    ),
  );
}

async function viewProjects() {
  await viewDashboard();
}

async function viewPage(title, load) {
  try {
    const body = await load();
    shell(
      el(
        "div",
        {},
        el("h1", {}, title),
        el("p", { className: "sub" }, "Live API"),
        body,
      ),
    );
  } catch (e) {
    shell(
      el(
        "div",
        {},
        el("h1", {}, title),
        el("p", { className: "err" }, String(e)),
      ),
    );
  }
}

async function viewDeploy() {
  await viewPage("Deploy", async () => {
    const data = await api("/api/artifacts");
    const arts = data.artifacts || [];
    if (!arts.length)
      return el(
        "p",
        { className: "sub" },
        "No local artifacts yet. Compile a project first.",
      );
    return el(
      "div",
      {},
      ...arts.map((a) =>
        el(
          "div",
          { className: "card" },
          el("div", { className: "name" }, a.repo || a.name || "?"),
          el(
            "div",
            { className: "meta" },
            a.path || a.artifact_dir || JSON.stringify(a),
          ),
        ),
      ),
    );
  });
}

async function viewRegistry() {
  await viewPage("Registry", async () => {
    const data = await api("/api/layers");
    const layers = data.layers || [];
    if (!layers.length)
      return el(
        "p",
        { className: "sub" },
        "No layers found (set VEIL_LAYERS_DIR or use monorepo layers/).",
      );
    return el(
      "div",
      {},
      ...layers.map((l) =>
        el(
          "div",
          { className: "card" },
          el("div", { className: "name" }, l.name || l.id || "?"),
          el("div", { className: "meta" }, l.path || l.kind || ""),
        ),
      ),
    );
  });
}

async function viewBus() {
  const out = el("pre", { className: "sub" }, "");
  const typeIn = el("input", { value: "ListRepos", id: "busType" });
  const go = el(
    "button",
    {
      type: "button",
      onClick: async () => {
        try {
          const message = { type: typeIn.value || "ListRepos" };
          const r = await api("/bus/invoke", {
            method: "POST",
            body: JSON.stringify({ message }),
          });
          out.textContent = JSON.stringify(r, null, 2);
        } catch (e) {
          out.textContent = String(e);
        }
      },
    },
    "Invoke",
  );
  shell(
    el(
      "div",
      {},
      el("h1", {}, "Bus"),
      el(
        "p",
        { className: "sub" },
        "POST /bus/invoke — generated storage handlers",
      ),
      el("label", {}, "message.type"),
      el("div", { className: "row" }, typeIn, go),
      out,
    ),
  );
}

async function viewAgents() {
  shell(
    el(
      "div",
      {},
      el("h1", {}, "Agents"),
      el(
        "p",
        { className: "sub" },
        "Full ACP turns run in the dual-loop IDE agent dock.",
      ),
      el("p", {}, "Open a project IDE, then use the agent panel:"),
      el("code", {}, "POST /api/p/{project}/agent/turn"),
      el(
        "p",
        { className: "sub" },
        "Bus HandleAgentMessage returns a pointer to that path.",
      ),
    ),
  );
}

async function viewConfig() {
  let cfg = {};
  try {
    cfg = await api("/api/config");
  } catch (e) {
    shell(
      el(
        "div",
        {},
        el("h1", {}, "Config"),
        el("p", { className: "err" }, String(e)),
      ),
    );
    return;
  }
  const input = el("input", { value: cfg.projects_dir || "", id: "pd" });
  const status = el("p", { className: "sub" }, "");
  const save = el(
    "button",
    {
      type: "button",
      onClick: async () => {
        try {
          const body = { projects_dir: input.value };
          const r = await api("/api/config", {
            method: "PATCH",
            body: JSON.stringify(body),
          });
          status.textContent = r.ok === false ? r.error || "failed" : "Saved.";
        } catch (e) {
          status.textContent = String(e);
        }
      },
    },
    "Save projects_dir",
  );
  shell(
    el(
      "div",
      {},
      el("h1", {}, "Config"),
      el("p", { className: "sub" }, cfg.config_path || ""),
      el("label", {}, "projects_dir"),
      el("div", { className: "row" }, input, save),
      status,
    ),
  );
}

function route() {
  // Strip trailing slash without a regex (avoids codegen escape bugs).
  let p = location.pathname || "/";
  while (p.length > 1 && p.endsWith("/")) p = p.slice(0, -1);
  if (p === "/config" || p.startsWith("/config/")) return viewConfig();
  if (p === "/deploy" || p.startsWith("/deploy/")) return viewDeploy();
  if (p === "/registry" || p.startsWith("/registry/")) return viewRegistry();
  if (p === "/bus" || p.startsWith("/bus/")) return viewBus();
  if (p === "/agents" || p.startsWith("/agents/")) return viewAgents();
  if (p === "/projects" || p.startsWith("/projects/")) {
    // /projects/{name}/ide is a full HTML page (iframe), not SPA
    if (p.includes("/ide")) return;
    return viewProjects();
  }
  return viewDashboard();
}

route();
window.addEventListener("popstate", route);
