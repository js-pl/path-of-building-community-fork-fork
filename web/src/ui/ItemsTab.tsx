// Items tab: the build's slots and items, and upstream's item editor (the
// "display item") with its own controls: variants, crafted affixes, quality,
// influences, catalysts, cluster jewels, enchants, anoints and corruption.
// Items come from their text, upstream's crafting base picker, or upstream's
// unique and rare templates.

import { useEffect, useMemo, useState } from "preact/hooks";
import type { TabProps } from "../app";
import { engine } from "../app";
import type { DisplayItem, Item, Items, TooltipLine } from "../engine/types";
import { ColorText } from "./ColorText";
import { ControlView } from "./Control";
import { TooltipView } from "./Tooltip";

type Template = { name: string; type: string; base?: string; raw: string };

const rarityColor: Record<string, string> = {
  NORMAL: "#c8c8c8",
  MAGIC: "#8888ff",
  RARE: "#ffff77",
  UNIQUE: "#af6025",
  RELIC: "#60c060",
};

export function ItemsTab({ act, rev, setControl }: TabProps) {
  const [items, setItems] = useState<Items | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [tip, setTip] = useState<TooltipLine[] | null>(null);
  const [text, setText] = useState("");
  const [display, setDisplay] = useState<DisplayItem | null>(null);
  const [db, setDb] = useState<{ kind: string; list: Template[] } | null>(null);
  const [dbSearch, setDbSearch] = useState("");

  useEffect(() => {
    engine.call<Items>("items").then(setItems);
  }, [rev]);

  useEffect(() => {
    if (selected === null) return setTip(null);
    engine
      .call<{ lines: TooltipLine[] }>("itemTooltip", { id: selected })
      .then((t) => setTip(t.lines))
      .catch(() => setTip(null));
  }, [selected, rev]);

  useEffect(() => {
    engine.call<DisplayItem>("displayItem").then(setDisplay);
  }, [rev]);

  const byId = useMemo(() => new Map((items?.items ?? []).map((i) => [i.id, i])), [items]);

  if (!items) return <p>Loading...</p>;

  const openDb = async (kind: string) => {
    if (db?.kind === kind) return setDb(null);
    setDb({ kind, list: await engine.call<Template[]>("itemDb", { kind }) });
  };
  const q = dbSearch.trim().toLowerCase();
  const dbList = db ? db.list.filter((t) => !q || t.name.toLowerCase().includes(q) || (t.base ?? "").toLowerCase().includes(q)).slice(0, 200) : [];

  return (
    <div class="items-tab">
      <div class="slots">
        {items.itemSets.length > 1 ? (
          <select value={String(items.activeItemSet)} onChange={(e) => act("selectItemSet", { id: Number((e.target as HTMLSelectElement).value) })}>
            {items.itemSets.map((s) => (
              <option key={s.id} value={String(s.id)}>
                {s.title}
              </option>
            ))}
          </select>
        ) : null}
        <table>
          <tbody>
            {items.slots
              .filter((s) => s.shown)
              .map((s) => (
                <tr key={s.name}>
                  <th>{s.label}</th>
                  <td>
                    <select
                      value={String(s.item ?? 0)}
                      onChange={(e) => act("equipItem", { slot: s.name, id: Number((e.target as HTMLSelectElement).value) })}
                      onFocus={() => s.item && setSelected(s.item)}
                    >
                      <option value="0">None</option>
                      {s.candidates.map((id) => (
                        <option key={id} value={String(id)}>
                          {byId.get(id)?.name ?? id}
                        </option>
                      ))}
                    </select>
                  </td>
                </tr>
              ))}
          </tbody>
        </table>
      </div>
      <div class="item-list">
        <h3>All items</h3>
        <ul>
          {items.items.map((i) => (
            <li key={i.id} class={selected === i.id ? "selected" : ""} onClick={() => setSelected(i.id)} style={{ color: rarityColor[i.rarity] }}>
              {i.name}
            </li>
          ))}
        </ul>
        {selected !== null && byId.get(selected) ? (
          <ItemActions
            item={byId.get(selected)!}
            onEdit={(it) => act("setDisplayItem", { id: it.id })}
            onDelete={async (it) => {
              await act("deleteItem", { id: it.id });
              setSelected(null);
            }}
          />
        ) : null}
        {tip ? <TooltipView lines={tip} /> : null}
      </div>
      <div class="item-editor">
        <div class="buttons">
          <button onClick={() => act("craftItem")}>Craft item...</button>
          <button onClick={() => openDb("unique")}>Uniques...</button>
          <button onClick={() => openDb("rare")}>Rare templates...</button>
        </div>
        {db ? (
          <div class="item-db">
            <input type="search" placeholder={`Search ${db.kind === "rare" ? "rare templates" : "uniques"}`} value={dbSearch} onInput={(e) => setDbSearch((e.target as HTMLInputElement).value)} />
            <ul>
              {dbList.map((t, i) => (
                <li
                  key={i}
                  onClick={async () => {
                    setDb(null);
                    await act("setDisplayItem", { text: t.raw });
                  }}
                >
                  <ColorText text={t.name} /> <span class="base">{t.base}</span>
                </li>
              ))}
            </ul>
          </div>
        ) : null}
        {display?.item ? (
          <div class="display-item">
            <h3>{display.existing ? "Editing item" : "New item"}</h3>
            <div class="buttons">
              <button onClick={() => act("saveDisplayItem")}>{display.existing ? "Save changes" : "Add to build"}</button>
              <button onClick={() => navigator.clipboard?.writeText(display.item!.raw)}>Copy text</button>
              <button onClick={() => act("setDisplayItem")}>Close</button>
            </div>
            <div class="display-controls">
              {(display.controls ?? []).map((c) => (
                <ControlView key={c.path} path={c.path!} c={c} set={setControl} />
              ))}
            </div>
            {display.lines ? <TooltipView lines={display.lines} /> : null}
          </div>
        ) : (
          <>
            <p>Paste an item copied in game (Ctrl+C), or write one.</p>
            <textarea rows={12} value={text} onInput={(e) => setText((e.target as HTMLTextAreaElement).value)} />
            <div class="buttons">
              <button
                onClick={async () => {
                  await act("setDisplayItem", { text });
                  setText("");
                }}
              >
                Open in editor
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function ItemActions({ item, onEdit, onDelete }: { item: Item; onEdit: (i: Item) => void; onDelete: (i: Item) => void }) {
  return (
    <div class="buttons">
      <button onClick={() => onEdit(item)}>Edit</button>
      <button onClick={() => navigator.clipboard?.writeText(item.raw)}>Copy</button>
      <button onClick={() => onDelete(item)}>Delete</button>
    </div>
  );
}
