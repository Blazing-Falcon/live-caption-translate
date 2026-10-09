// Minimal runtime so the mockup boards open in a normal browser (file:// is fine).
// Supports what the boards use: a DCLogic component with props/state/renderVals, {{expr}} in text and
// attributes, <sc-if value>, <sc-for list as>, onClick handlers, and <helmet> for head content.
// Props can be overridden from the URL: Main.dc.html?mode=Move&barOpacity=60 ; Controls.dc.html?tab=perf&source=apps
(function () {
  class DCLogic {
    constructor(props) { this.props = props || {}; this.state = {}; }
    setState(patch) { this.state = Object.assign({}, this.state, patch); render(); }
    renderVals() { return {}; }
  }
  window.DCLogic = DCLogic;
  let comp, template, root;

  function lookup(expr, scope) {
    expr = expr.trim();
    if (expr === 'true') return true;
    if (expr === 'false') return false;
    return expr.split('.').reduce((o, k) => (o == null ? undefined : o[k]), scope);
  }
  function subst(str, scope) {
    return str.replace(/\{\{([^}]*)\}\}/g, (_, e) => { const v = lookup(e, scope); return v == null ? '' : String(v); });
  }
  function single(str) { const m = /^\s*\{\{([^}]*)\}\}\s*$/.exec(str); return m ? m[1] : null; }

  function walk(node, scope) {
    if (node.nodeType === 3) { if (node.nodeValue.includes('{{')) node.nodeValue = subst(node.nodeValue, scope); return; }
    if (node.nodeType !== 1) return;
    const tag = node.tagName.toLowerCase();
    if (tag === 'sc-if') {
      const v = lookup(single(node.getAttribute('value')) || 'false', scope);
      if (!v) { node.remove(); return; }
      const kids = [...node.childNodes]; node.replaceWith(...kids); kids.forEach((k) => walk(k, scope)); return;
    }
    if (tag === 'sc-for') {
      const list = lookup(single(node.getAttribute('list')) || '', scope) || [];
      const as = node.getAttribute('as');
      const frag = [];
      list.forEach((item) => {
        const s = Object.assign({}, scope, { [as]: item });
        [...node.childNodes].forEach((c) => { const cl = c.cloneNode(true); frag.push([cl, s]); });
      });
      node.replaceWith(...frag.map((f) => f[0])); frag.forEach(([n, s]) => walk(n, s)); return;
    }
    for (const a of [...node.attributes]) {
      if (!a.value.includes('{{')) continue;
      if (/^on/i.test(a.name)) {
        const fn = lookup(single(a.value) || '', scope); node.removeAttribute(a.name);
        if (typeof fn === 'function') node.addEventListener(a.name.slice(2).toLowerCase(), fn);
      } else node.setAttribute(a.name, subst(a.value, scope));
    }
    [...node.childNodes].forEach((c) => walk(c, scope));
  }

  function render() {
    const vals = comp.renderVals();
    const tmp = document.createElement('div'); tmp.innerHTML = template;
    [...tmp.childNodes].forEach((c) => walk(c, vals));
    root.replaceChildren(...tmp.childNodes);
  }

  document.addEventListener('DOMContentLoaded', () => {
    const x = document.querySelector('x-dc'); if (!x) return;
    const helmet = x.querySelector('helmet');
    if (helmet) { [...helmet.childNodes].forEach((n) => document.head.appendChild(n)); helmet.remove(); }
    const script = document.querySelector('script[data-dc-script]');
    const meta = JSON.parse(script.getAttribute('data-props') || '{}');
    script.remove();
    const props = {};
    for (const [k, v] of Object.entries(meta)) if (!k.startsWith('$') && v && 'default' in v) props[k] = v.default;
    const q = new URLSearchParams(location.search);
    const Cls = new Function('DCLogic', script.textContent + '\nreturn Component;')(DCLogic);
    for (const [k, v] of q) props[k] = v === 'true' ? true : v === 'false' ? false : isNaN(+v) ? v : +v;
    comp = new Cls(props);
    for (const [k, v] of q) if (comp.state && k in comp.state) comp.state[k] = props[k];
    template = x.innerHTML; root = x; render();
  });
})();
