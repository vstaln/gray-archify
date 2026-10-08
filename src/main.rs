//! gray-archify — repo → one self-contained interactive HTML module map.
//!
//! Behavioral port of tt-a1i/archify (MIT) scoped to the agent-facing core:
//! walk a repository (≤500 files), parse import/include lines into a module
//! graph, and emit ONE standalone HTML artifact — an expandable directory
//! tree plus a force-layout SVG graph driven by vendored inline JS (no
//! external libs). `archify` writes the file; `archify_prompt` returns the
//! same walk as a compact markdown outline the model can reason over.
//!
//! Wire: `tool/call` only; protocol 1.1.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

const FILE_CAP: usize = 500;

fn manifest() -> Value {
    json!({
        "name": "archify",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": "1.1",
        "tools": [
            {
                "name": "archify",
                "description": "Map a repository's architecture into ONE self-contained \
                    interactive HTML file: an expandable directory tree (sizes/loc) plus a \
                    force-layout SVG module graph built from parsed import/include edges. \
                    No external assets — the file opens offline. Returns a one-line receipt.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Repo root; default \".\"." },
                        "depth": { "type": "integer", "description": "Max directory depth walked; default 2." },
                        "out": { "type": "string", "description": "Output HTML path; default \"./archify.html\"." }
                    }
                }
            },
            {
                "name": "archify_prompt",
                "description": "Return a compact markdown module map of the repository — \
                    directory tree with per-file loc and resolved import targets — so you \
                    can reason about architecture before answering `question`.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "question": { "type": "string", "description": "What you want to understand about the repo." },
                        "path": { "type": "string", "description": "Repo root; default \".\"." },
                        "depth": { "type": "integer", "description": "Max directory depth walked; default 2." }
                    },
                    "required": ["question"]
                }
            }
        ],
        "commands": ["/archify"],
    })
}

// --- import extraction --------------------------------------------------------

static RE_JS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:import|export)[^'"]*?from\s*['"]([^'"]+)['"]|import\s*['"]([^'"]+)['"]|require\(\s*['"]([^'"]+)['"]"#).unwrap()
});
static RE_RS_USE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\buse\s+([a-zA-Z0-9_:]+)").unwrap());
static RE_RS_MOD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bmod\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*;").unwrap());
static RE_PY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*from\s+([.\w]+)\s+import|^\s*import\s+([.\w]+)").unwrap()
});
static RE_GO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*(?:[\w.]+\s+)?"([\w./-]+)"\s*$|import\s+"([\w./-]+)""#).unwrap());
static RE_C: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"#\s*include\s*"([^"]+)""#).unwrap());
static RE_JAVA: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*import\s+(?:static\s+)?([\w.]+)\s*;").unwrap());

fn lang_of(ext: &str) -> &'static str {
    match ext {
        "rs" => "rust",
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => "js",
        "py" => "python",
        "go" => "go",
        "c" | "h" | "cpp" | "cc" | "hpp" | "cxx" | "hxx" => "c",
        "java" | "kt" => "java",
        _ => "",
    }
}

const CODE_EXTS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "c", "h", "cpp", "cc", "hpp",
    "cxx", "hxx", "java", "kt", "rb", "php", "cs", "swift", "scala", "vue", "svelte",
];

/// Extract raw import specifiers + whether each is explicitly relative.
fn imports_of(ext: &str, content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |s: &str| {
        if !s.is_empty() && out.len() < 64 {
            out.push(s.to_string());
        }
    };
    match lang_of(ext) {
        "js" => {
            for c in RE_JS.captures_iter(content) {
                for g in c.iter().flatten() {
                    push(g.as_str());
                }
            }
        }
        "rust" => {
            for c in RE_RS_USE.captures_iter(content) {
                push(&c[1]);
            }
            for c in RE_RS_MOD.captures_iter(content) {
                push(&format!("mod:{}", &c[1]));
            }
        }
        "python" => {
            for c in RE_PY.captures_iter(content) {
                for g in c.iter().flatten() {
                    push(g.as_str());
                }
            }
        }
        "go" => {
            for c in RE_GO.captures_iter(content) {
                for g in c.iter().flatten() {
                    push(g.as_str());
                }
            }
        }
        "c" => {
            for c in RE_C.captures_iter(content) {
                push(&c[1]);
            }
        }
        "java" => {
            for c in RE_JAVA.captures_iter(content) {
                push(&c[1]);
            }
        }
        _ => {}
    }
    out
}

// --- repo walk ----------------------------------------------------------------

#[derive(Debug)]
struct Mod {
    rel: String,          // repo-relative path, '/'-separated
    dir: String,          // repo-relative dir ("" for root)
    ext: String,
    loc: usize,
    size: u64,
    specs: Vec<String>,
    edges: Vec<usize>,    // resolved indices into `mods`
}

const SKIP_DIRS: &[&str] = &[
    "node_modules", ".git", "target", "dist", "build", ".next", ".nuxt", "coverage",
    "__pycache__", "vendor", ".gray", ".venv", "venv", "out", ".turbo", ".cache",
];

fn walk(dir: &Path, root: &Path, depth: usize, max_depth: usize, out: &mut Vec<Mod>) {
    if out.len() >= FILE_CAP || depth > max_depth {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if out.len() >= FILE_CAP {
            return;
        }
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_str()) {
                walk(&p, root, depth + 1, max_depth, out);
            }
        } else if !name.starts_with('.') {
            let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_string();
            let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            let (loc, specs) = if CODE_EXTS.contains(&ext.as_str()) {
                match std::fs::read_to_string(&p) {
                    Ok(c) => (c.lines().count(), imports_of(&ext, &c)),
                    Err(_) => (0, Vec::new()),
                }
            } else {
                (0, Vec::new())
            };
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            let dir_rel = Path::new(&rel).parent().map(|d| d.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            out.push(Mod { rel, dir: dir_rel, ext, loc, size, specs, edges: Vec::new() });
        }
    }
}

/// Spec → repo-relative candidates, then suffix-match against walked files.
fn resolve(spec: &str, from: &Mod, ext: &str, files: &HashSet<String>) -> Option<String> {
    let mut cands: Vec<String> = Vec::new();
    let exts: &[&str] = match lang_of(ext) {
        "rust" => &["rs"],
        "js" => &["ts", "tsx", "js", "jsx", "mjs", "cjs"],
        "python" => &["py"],
        "go" => &["go"],
        "c" => &["c", "h", "cpp", "cc", "hpp", "cxx", "hxx"],
        "java" => &["java", "kt"],
        _ => &["ts", "js", "py", "rs", "go"],
    };
    let join_exts = |base: &str, cands: &mut Vec<String>| {
        cands.push(base.to_string());
        for e in exts {
            cands.push(format!("{base}.{e}"));
            cands.push(format!("{base}/index.{e}"));
            cands.push(format!("{base}/mod.{e}"));
            cands.push(format!("{base}/__init__.{e}"));
        }
    };
    if let Some(m) = spec.strip_prefix("mod:") {
        join_exts(&format!("{}/{m}", from.dir), &mut cands);
    } else if spec.starts_with('.') || spec.starts_with('/') {
        let base = if spec.starts_with('.') {
            let mut d = PathBuf::from(&from.dir);
            for part in Path::new(spec).components() {
                use std::path::Component::*;
                match part {
                    CurDir => {}
                    ParentDir => { d.pop(); }
                    Normal(s) => d.push(s),
                    _ => {}
                }
            }
            d.to_string_lossy().replace('\\', "/")
        } else {
            spec.trim_start_matches('/').to_string()
        };
        join_exts(&base, &mut cands);
    } else if lang_of(ext) == "rust" {
        let mut parts: Vec<&str> = spec.split("::").collect();
        let mut dir = from.dir.clone();
        while matches!(parts.first(), Some(&"self" | &"super" | &"crate")) {
            match parts.remove(0) {
                "self" => {}
                "super" => {
                    dir = Path::new(&dir)
                        .parent()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default();
                }
                _ => dir.clear(), // crate → repo root-ish
            }
        }
        let s = parts.join("/");
        if !s.is_empty() {
            join_exts(&format!("{dir}/{s}").trim_matches('/').to_string(), &mut cands);
            join_exts(&format!("src/{s}"), &mut cands);
            join_exts(&s, &mut cands);
            // `use a::b::item` — the last segment is usually an item, not a module
            if let Some((head, _)) = s.rsplit_once('/') {
                join_exts(&format!("{dir}/{head}").trim_matches('/').to_string(), &mut cands);
                join_exts(&format!("src/{head}"), &mut cands);
                join_exts(head, &mut cands);
            }
        }
    } else {
        // dotted or package-ish: a.b.c → a/b/c, plus progressively shorter
        let s = spec.replace('.', "/").replace("::", "/");
        join_exts(&s, &mut cands);
        if let Some((head, _)) = s.rsplit_once('/') {
            join_exts(head, &mut cands);
        }
    }
    for cand in &cands {
        let cand = cand.trim_start_matches("./").trim_matches('/');
        if cand.is_empty() {
            continue;
        }
        if files.contains(cand) {
            return Some(cand.to_string());
        }
        // suffix match: import specifiers are often package-relative
        if let Some(hit) = files
            .iter()
            .filter(|f| f.ends_with(&format!("/{cand}")) || f.as_str() == cand)
            .min_by_key(|f| f.len())
        {
            return Some(hit.clone());
        }
    }
    None
}

fn build_graph(mods: &mut Vec<Mod>) -> usize {
    let files: HashSet<String> = mods.iter().map(|m| m.rel.clone()).collect();
    let index: HashMap<String, usize> =
        mods.iter().enumerate().map(|(i, m)| (m.rel.clone(), i)).collect();
    let mut resolved: Vec<Vec<usize>> = Vec::with_capacity(mods.len());
    for m in mods.iter() {
        let mut seen = HashSet::new();
        let mut targets = Vec::new();
        for spec in &m.specs {
            if let Some(target) = resolve(spec, m, &m.ext, &files)
                && let Some(&j) = index.get(target.as_str())
                && Some(&j) != index.get(m.rel.as_str())
                && seen.insert(j)
            {
                targets.push(j);
            }
        }
        resolved.push(targets);
    }
    let mut edge_count = 0;
    for (i, targets) in resolved.into_iter().enumerate() {
        for j in targets {
            if j != i {
                mods[i].edges.push(j);
                edge_count += 1;
            }
        }
    }
    edge_count
}

// --- HTML ---------------------------------------------------------------------

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn is_entry(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    matches!(
        name,
        "main.rs" | "main.go" | "main.py" | "__main__.py" | "index.ts" | "index.tsx"
            | "index.js" | "app.py" | "App.tsx" | "main.c" | "lib.rs" | "mod.rs"
    )
}

/// Nested `<details>` tree: dir → (subdirs, files).
fn render_tree(mods: &[Mod]) -> String {
    #[derive(Default)]
    struct Node {
        dirs: std::collections::BTreeMap<String, Node>,
        files: Vec<usize>,
    }
    let mut root = Node::default();
    for (i, m) in mods.iter().enumerate() {
        let mut n = &mut root;
        if !m.dir.is_empty() {
            for part in m.dir.split('/') {
                n = n.dirs.entry(part.to_string()).or_default();
            }
        }
        n.files.push(i);
    }
    fn rec(n: &Node, mods: &[Mod], s: &mut String) {
        for (name, sub) in &n.dirs {
            s.push_str(&format!(
                "<details><summary class=\"dir\">{}/</summary><div class=\"kids\">",
                html_escape(name)
            ));
            rec(sub, mods, s);
            s.push_str("</div></details>");
        }
        for &i in &n.files {
            let m = &mods[i];
            let fname = m.rel.rsplit('/').next().unwrap_or(&m.rel);
            let meta = if m.loc > 0 {
                format!("{} loc", m.loc)
            } else {
                format!("{} B", m.size)
            };
            let entry = if is_entry(&m.rel) { " <em>entry</em>" } else { "" };
            s.push_str(&format!(
                "<div class=\"file\" title=\"{}\">{fname}<span class=\"meta\">{meta}</span>{entry}</div>",
                html_escape(&m.rel),
                fname = html_escape(fname),
            ));
        }
    }
    let mut s = String::new();
    rec(&root, mods, &mut s);
    s
}

fn render_html(root: &Path, mods: &[Mod], edges: usize) -> String {
    let tree = render_tree(mods);
    let nodes: Vec<Value> = mods
        .iter()
        .map(|m| {
            json!({
                "id": m.rel, "name": m.rel.rsplit('/').next().unwrap_or(&m.rel),
                "dir": m.dir, "loc": m.loc, "ext": m.ext,
                "entry": is_entry(&m.rel),
            })
        })
        .collect();
    let mut links = Vec::new();
    for (i, m) in mods.iter().enumerate() {
        for &j in &m.edges {
            links.push(json!({"s": i, "t": j}));
        }
    }
    let mut data = json!({"nodes": nodes, "links": links}).to_string();
    data = data.replace('<', "\\u003c"); // keep the JSON inside <script> inert
    let title = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".into());
    format!(
        r##"<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>archify — {title}</title>
<style>
:root{{color-scheme:dark}}
body{{margin:0;font:14px/1.5 ui-monospace,Menlo,monospace;background:#0d1117;color:#e6edf3}}
header{{padding:14px 20px;border-bottom:1px solid #30363d;display:flex;gap:18px;align-items:baseline;flex-wrap:wrap}}
h1{{font-size:18px;margin:0;color:#58a6ff}}
.stat{{color:#8b949e}}
main{{display:flex;min-height:calc(100vh - 54px)}}
#tree{{width:340px;min-width:240px;overflow:auto;border-right:1px solid #30363d;padding:12px;resize:horizontal}}
#graph{{flex:1;position:relative}}
svg{{width:100%;height:100%;display:block}}
details{{margin-left:10px}}
summary.dir{{cursor:pointer;color:#d2a8ff;font-weight:600}}
.kids{{margin-left:12px;border-left:1px solid #21262d;padding-left:8px}}
.file{{color:#c9d1d9;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}}
.file em{{color:#f0883e;font-style:normal;font-size:11px}}
.meta{{color:#8b949e;font-size:11px;margin-left:8px}}
.node{{cursor:pointer}}
.node circle{{fill:#1f6feb;stroke:#58a6ff;stroke-width:1}}
.node.entry circle{{fill:#b35900;stroke:#f0883e}}
.node text{{fill:#c9d1d9;font-size:10px;pointer-events:none}}
.link{{stroke:#30363d;stroke-width:1}}
.link.hot{{stroke:#f0883e;stroke-width:2}}
.legend{{position:absolute;right:12px;top:12px;color:#8b949e;font-size:11px;text-align:right}}
</style></head><body>
<header><h1>archify</h1><span class="stat">{title}</span>
<span class="stat">{nfiles} modules</span><span class="stat">{edges} edges</span></header>
<main><nav id="tree">{tree}</nav>
<section id="graph"><svg id="svg"></svg>
<div class="legend">drag to arrange · click a node to light its imports<br>orange = entry point</div></section></main>
<script>
const DATA = {data};
const svg = document.getElementById('svg');
const W = () => svg.clientWidth || 900, H = () => svg.clientHeight || 600;
const nodes = DATA.nodes.map((n,i) => Object.assign({{x:W()/2+Math.cos(i)*200, y:H()/2+Math.sin(i)*200, vx:0, vy:0}}, n));
const links = DATA.links;
// tiny force layout: repulsion + springs + centering, fixed ticks
for (let t=0; t<350; t++) {{
  for (const a of nodes) {{
    a.vx += (W()/2-a.x)*0.002; a.vy += (H()/2-a.y)*0.002;
    for (const b of nodes) if (b!==a) {{
      let dx=a.x-b.x, dy=a.y-b.y, d2=dx*dx+dy*dy||1;
      if (d2<40000) {{ let f=1800/d2; a.vx+=dx*f/Math.sqrt(d2); a.vy+=dy*f/Math.sqrt(d2); }}
    }}
  }}
  for (const l of links) {{
    const a=nodes[l.s], b=nodes[l.t];
    let dx=b.x-a.x, dy=b.y-a.y, d=Math.hypot(dx,dy)||1;
    let f=(d-90)*0.01; a.vx+=dx/d*f; a.vy+=dy/d*f; b.vx-=dx/d*f; b.vy-=dy/d*f;
  }}
  for (const n of nodes) {{ n.x+=n.vx*=0.85; n.y+=n.vy*=0.85; }}
}}
const NS='http://www.w3.org/2000/svg';
const linkEls = links.map(l => {{
  const e=document.createElementNS(NS,'line'); e.setAttribute('class','link'); svg.appendChild(e); return e;
}});
const nodeEls = nodes.map((n,i) => {{
  const g=document.createElementNS(NS,'g'); g.setAttribute('class','node'+(n.entry?' entry':''));
  const c=document.createElementNS(NS,'circle');
  c.setAttribute('r', Math.max(4, Math.min(16, 4+Math.sqrt(n.loc||4)/3)));
  g.appendChild(c);
  const t=document.createElementNS(NS,'text'); t.setAttribute('x',10); t.setAttribute('y',4);
  t.textContent=n.name; g.appendChild(t);
  g.addEventListener('click', () => {{
    links.forEach((l,k) => linkEls[k].setAttribute('class', (l.s===i||l.t===i)?'link hot':'link'));
  }});
  g.addEventListener('pointerdown', ev => {{
    g.setPointerCapture(ev.pointerId);
    const mv=e2=>{{const r=svg.getBoundingClientRect(); n.x=e2.clientX-r.left; n.y=e2.clientY-r.top; draw();}};
    const up=()=>{{g.removeEventListener('pointermove',mv); g.removeEventListener('pointerup',up);}};
    g.addEventListener('pointermove',mv); g.addEventListener('pointerup',up);
  }});
  svg.appendChild(g); return g;
}});
function draw() {{
  links.forEach((l,k) => {{
    const a=nodes[l.s], b=nodes[l.t], e=linkEls[k];
    e.setAttribute('x1',a.x); e.setAttribute('y1',a.y); e.setAttribute('x2',b.x); e.setAttribute('y2',b.y);
  }});
  nodes.forEach((n,i) => nodeEls[i].setAttribute('transform',`translate(${{n.x}},${{n.y}})`));
}}
draw();
</script></body></html>"##,
        title = html_escape(&title),
        nfiles = mods.len(),
        edges = edges,
        tree = tree,
        data = data,
    )
}

// --- markdown outline (archify_prompt) ----------------------------------------

fn render_outline(root: &Path, mods: &[Mod], edges: usize, question: &str) -> String {
    let mut s = format!(
        "# Module map — {}\nquestion: {question}\n{} files, {} resolved import edges\n\n",
        root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| ".".into()),
        mods.len(),
        edges,
    );
    // group by directory, most files first
    let mut by_dir: HashMap<&str, Vec<&Mod>> = HashMap::new();
    for m in mods {
        by_dir.entry(m.dir.as_str()).or_default().push(m);
    }
    let mut dirs: Vec<_> = by_dir.keys().copied().collect();
    dirs.sort();
    for d in dirs {
        let mut ms = by_dir[d].clone();
        ms.sort_by(|a, b| b.loc.cmp(&a.loc));
        s.push_str(&format!("## {}\n", if d.is_empty() { "." } else { d }));
        for m in ms {
            let name = m.rel.rsplit('/').next().unwrap_or(&m.rel);
            let targets: Vec<&str> = m
                .edges
                .iter()
                .map(|&j| mods[j].rel.rsplit('/').next().unwrap_or(&mods[j].rel))
                .collect();
            let entry = if is_entry(&m.rel) { " [entry]" } else { "" };
            if targets.is_empty() {
                s.push_str(&format!("- {name} ({} loc){entry}\n", m.loc));
            } else {
                s.push_str(&format!("- {name} ({} loc){entry} → {}\n", m.loc, targets.join(", ")));
            }
        }
    }
    s
}

// --- tools --------------------------------------------------------------------

fn scan(args: &Value) -> Result<(PathBuf, Vec<Mod>, usize), String> {
    let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let depth = args.get("depth").and_then(Value::as_u64).unwrap_or(2) as usize;
    let root = PathBuf::from(path);
    if !root.is_dir() {
        return Err(format!("not a directory: {path}"));
    }
    let mut mods = Vec::new();
    walk(&root, &root, 0, depth, &mut mods);
    let edges = build_graph(&mut mods);
    Ok((root, mods, edges))
}

fn call_tool(name: &str, args: &Value) -> Result<String, String> {
    match name {
        "archify" => {
            let (root, mods, edges) = scan(args)?;
            let out = args.get("out").and_then(Value::as_str).unwrap_or("./archify.html");
            let out_path = PathBuf::from(out);
            let out_path = if out_path.is_absolute() {
                out_path
            } else {
                std::env::current_dir().map_err(|e| e.to_string())?.join(out_path)
            };
            let html = render_html(&root, &mods, edges);
            std::fs::write(&out_path, &html).map_err(|e| format!("couldn't write {out}: {e}"))?;
            Ok(format!("wrote {} ({} modules, {} edges)", out_path.display(), mods.len(), edges))
        }
        "archify_prompt" => {
            let question = args.get("question").and_then(Value::as_str).unwrap_or("");
            if question.trim().is_empty() {
                return Err("missing required argument: question".into());
            }
            let (root, mods, edges) = scan(args)?;
            let mut s = render_outline(&root, &mods, edges, question);
            s.truncate(8000);
            Ok(s)
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

fn run_command(argv: &[&str]) -> String {
    let _ = argv;
    format!(
        "gray-archify {} — tools: `archify` writes an interactive HTML module map; \
        `archify_prompt` returns a markdown outline for reasoning",
        env!("CARGO_PKG_VERSION")
    )
}

/// One request → `Some(reply)`, or `None` for notifications. The bool asks
/// the loop to exit after writing the reply.
fn handle(req: &Value) -> (Option<Value>, bool) {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = id else {
        return (None, method == "plugin/shutdown");
    };
    let result = match method {
        "plugin/manifest" => manifest(),
        "tool/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("args").cloned().unwrap_or(Value::Null);
            match call_tool(name, &args) {
                Ok(text) => json!({ "content": text }),
                Err(text) => json!({ "content": text, "is_error": true }),
            }
        }
        "command/run" => {
            let argv: Vec<&str> = params
                .get("argv")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            json!({ "text": run_command(&argv) })
        }
        "plugin/shutdown" => return (Some(json!({ "id": id, "result": {} })), true),
        _ => {
            let error = json!({ "code": -32601, "message": "method not found" });
            return (Some(json!({ "id": id, "error": error })), false);
        }
    };
    (Some(json!({ "id": id, "result": result })), false)
}

fn main() -> std::io::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("manifest") {
        println!("{}", manifest());
        return Ok(());
    }
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        let Ok(req) = serde_json::from_str::<Value>(&line) else { continue };
        let (reply, exit) = handle(&req);
        if let Some(reply) = reply {
            writeln!(stdout, "{reply}")?;
            stdout.flush()?;
        }
        if exit {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(method: &str, params: Value) -> Value {
        handle(&json!({ "id": 1, "method": method, "params": params })).0.unwrap()
    }

    fn fixture(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("archify-test-{}-{tag}", std::process::id()));
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("main.rs"), "mod store;\nuse crate::cli::run;\nfn main() {}\n").unwrap();
        std::fs::write(src.join("store.rs"), "pub struct S;\n").unwrap();
        std::fs::create_dir_all(src.join("cli")).unwrap();
        std::fs::write(src.join("cli/mod.rs"), "pub fn run() {}\n").unwrap();
        dir
    }

    #[test]
    fn manifest_has_two_tools() {
        let m = call("plugin/manifest", Value::Null)["result"].clone();
        assert_eq!(m["name"], "archify");
        let names: Vec<_> = m["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["archify", "archify_prompt"]);
    }

    #[test]
    fn rust_imports_resolve_to_edges() {
        let dir = fixture("graph");
        let mut mods = Vec::new();
        walk(&dir, &dir, 0, 4, &mut mods);
        let edges = build_graph(&mut mods);
        assert_eq!(mods.len(), 3);
        assert_eq!(edges, 2, "main.rs should edge to store.rs and cli/mod.rs");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn archify_writes_html_and_reports_counts() {
        let dir = fixture("html");
        let out = dir.join("map.html");
        let r = call("tool/call", json!({
            "name": "archify",
            "args": {"path": dir.to_str().unwrap(), "depth": 4, "out": out.to_str().unwrap()}
        }));
        let c = r["result"]["content"].as_str().unwrap();
        assert!(c.contains("3 modules") && c.contains("2 edges"), "{c}");
        let html = std::fs::read_to_string(&out).unwrap();
        assert!(html.contains("<details>") && html.contains("<svg") && !html.contains("<script src"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn archify_prompt_returns_markdown_map() {
        let dir = fixture("outline");
        let r = call("tool/call", json!({
            "name": "archify_prompt",
            "args": {"question": "where is state?", "path": dir.to_str().unwrap(), "depth": 4}
        }));
        let c = r["result"]["content"].as_str().unwrap();
        assert!(c.contains("# Module map") && c.contains("store.rs") && c.contains("→"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_path_is_tool_error_not_panic() {
        let r = call("tool/call", json!({"name": "archify", "args": {"path": "/nonexistent-xyz"}}));
        assert_eq!(r["result"]["is_error"], true);
    }

    #[test]
    fn html_escapes_names() {
        assert_eq!(html_escape("<b>&\"x\""), "&lt;b&gt;&amp;&quot;x&quot;");
    }
}
