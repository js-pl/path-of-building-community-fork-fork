// Text with upstream's color escapes: ^0-^9 and ^xRRGGBB

const digits = ["#000000", "#ff0000", "#00ff00", "#0000ff", "#ffff00", "#ff00ff", "#00ffff", "#ffffff", "#b3b3b3", "#666666"];

export type Span = { color?: string; text: string };

export function parseColors(text: string, initial?: string): Span[] {
  const out: Span[] = [];
  let color = initial;
  const re = /\^(x[0-9a-fA-F]{6}|[0-9])/g;
  let last = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    if (m.index > last) out.push({ color, text: text.slice(last, m.index) });
    const code = m[1];
    color = code.length === 1 ? digits[Number(code)] : "#" + code.slice(1);
    last = re.lastIndex;
  }
  if (last < text.length) out.push({ color, text: text.slice(last) });
  return out;
}

export function stripColors(text: string): string {
  return text.replace(/\^(x[0-9a-fA-F]{6}|[0-9])/g, "");
}

export function ColorText({ text, class: cls }: { text?: string | null; class?: string }) {
  if (!text) return null;
  return (
    <span class={cls}>
      {parseColors(text).map((s, i) => (
        <span key={i} style={s.color ? { color: s.color } : undefined}>
          {s.text}
        </span>
      ))}
    </span>
  );
}
