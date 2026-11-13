import { useEffect, useState } from "preact/hooks";
import type { TabProps } from "../app";
import { engine } from "../app";
import { isNotPorted } from "../engine/client";
import { NotPorted } from "./NotPorted";

export function NotesTab({ act }: TabProps) {
  const [text, setText] = useState<string | null>(null);
  const [unported, setUnported] = useState(false);
  useEffect(() => {
    engine
      .call<{ text: string }>("notes")
      .then((n) => setText(n.text))
      .catch((e) => setUnported(isNotPorted(e)));
  }, []);
  if (unported) return <NotPorted what="notes" />;
  if (text === null) return <p>Loading...</p>;
  return (
    <div class="notes-tab">
      <textarea value={text} onInput={(e) => setText((e.target as HTMLTextAreaElement).value)} onBlur={() => act("setNotes", { text })} />
    </div>
  );
}
