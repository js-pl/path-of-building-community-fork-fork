// Reads a Lua file that returns one table literal (upstream's tree.lua and
// sprites.lua) into JSON values. Tables with only implicit or 1..n keys
// become arrays, all others objects with string keys.

export function parseLua(src) {
  let i = 0;
  const n = src.length;

  const skip = () => {
    for (;;) {
      while (i < n && /\s/.test(src[i])) i++;
      if (src.startsWith("--", i)) {
        if (src.startsWith("--[[", i)) {
          const e = src.indexOf("]]", i);
          i = e < 0 ? n : e + 2;
        } else {
          const e = src.indexOf("\n", i);
          i = e < 0 ? n : e + 1;
        }
      } else return;
    }
  };

  const string = () => {
    const q = src[i++];
    let s = "";
    while (i < n && src[i] !== q) {
      if (src[i] === "\\") {
        const c = src[++i];
        if (c === "n") s += "\n";
        else if (c === "t") s += "\t";
        else if (/\d/.test(c)) {
          let d = "";
          while (d.length < 3 && /\d/.test(src[i])) d += src[i++];
          s += String.fromCharCode(Number(d));
          continue;
        } else s += c;
        i++;
      } else s += src[i++];
    }
    i++;
    return s;
  };

  const word = () => {
    const m = /^[-+]?(0x[0-9a-fA-F]+|\d*\.?\d+(?:[eE][-+]?\d+)?)|^[A-Za-z_]\w*/.exec(src.slice(i, i + 64));
    if (!m) throw new Error(`Lua literal: unexpected ${JSON.stringify(src.slice(i, i + 20))} at ${i}`);
    i += m[0].length;
    return m[0];
  };

  const value = () => {
    skip();
    const c = src[i];
    if (c === "{") return table();
    if (c === '"' || c === "'") return string();
    const w = word();
    if (w === "true") return true;
    if (w === "false") return false;
    if (w === "nil") return null;
    return Number(w);
  };

  const table = () => {
    i++;
    const entries = [];
    let next = 1;
    let array = true;
    for (;;) {
      skip();
      if (src[i] === "}") {
        i++;
        break;
      }
      let key;
      if (src[i] === "[") {
        i++;
        key = value();
        skip();
        i++; // ]
        skip();
        i++; // =
      } else {
        const save = i;
        const m = /^[A-Za-z_]\w*\s*=(?!=)/.exec(src.slice(i, i + 80));
        if (m && !/^(true|false|nil)\b/.test(m[0])) {
          key = m[0].replace(/\s*=$/, "");
          i += m[0].length;
        } else i = save;
      }
      const v = value();
      if (key === undefined) key = next;
      if (key !== next) array = false;
      else next++;
      entries.push([key, v]);
      skip();
      if (src[i] === "," || src[i] === ";") i++;
    }
    if (array) return entries.map((e) => e[1]);
    const o = {};
    for (const [k, v] of entries) o[String(k)] = v;
    return o;
  };

  skip();
  if (src.startsWith("return", i)) i += 6;
  return value();
}
