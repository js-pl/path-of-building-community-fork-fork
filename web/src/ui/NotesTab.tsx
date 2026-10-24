import { useEffect, useState } from "preact/hooks";
import type { TabProps } from "../app";
import { engine } from "../app";

export function NotesTab({ act }: TabProps) {
  const [text, setText] = useState<string | null>(null);
  useEffect(() => {
    engine.call<{ text: string }>("notes").then((n) => setText(n.text));
  }, []);
  if (text === null) return <p>Loading...</p>;
  return (
    <div class="notes-tab">
      <textarea value={text} onInput={(e) => setText((e.target as HTMLTextAreaElement).value)} onBlur={() => act("setNotes", { text })} />
    </div>
  );
}
