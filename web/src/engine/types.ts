// Responses of the engine API (crates/session/lua/session.lua)

export type Control = {
  class: string;
  shown: boolean;
  enabled: boolean;
  label?: string;
  list?: string[];
  sel?: number;
  text?: string;
  placeholder?: string;
  multiline?: boolean;
  state?: boolean;
  val?: number;
  tooltip?: string;
  path?: string;
};

export type Popup = { title?: string; lines: string[]; buttons: { name: string; label: string; enabled: boolean }[]; controls: Control[] };

export type StatRow = { 0?: string; 1?: string; gap?: boolean; center?: boolean };

export type State = {
  name: string;
  level: number;
  className: string;
  ascendClassName?: string;
  treeVersion: string;
  points: { used: number; ascendancy: number };
  pointsText?: string;
  stats: StatRow[];
  warnings: string[];
  controls: Record<string, Control>;
  popups: Popup[];
  unsaved: boolean;
  outputRevision: number;
};

export type TreeNode = {
  id: number;
  name: string;
  kind: string;
  x: number;
  y: number;
  ascendancy?: string;
  stats: string[];
  classStart?: string;
  mastery?: boolean;
};

export type Connector = { a: number; b: number; ascendancy?: string; arc?: { x: number; y: number; r: number } };

export type Tree = {
  version: string;
  bounds: { minX: number; minY: number; maxX: number; maxY: number };
  nodes: TreeNode[];
  connectors: Connector[];
  classes: { id: number; name: string; ascendancies: { id: number; name: string }[] }[];
};

export type TreeState = {
  alloc: number[];
  granted: number[];
  masteries: Record<string, number>;
  classId: number;
  ascendClassId: number;
  ascendancy?: string;
  secondaryAscendancy?: string;
};

export type TooltipLine = { size?: number; text?: string; separator?: boolean };

export type NodePower = {
  combined: boolean;
  max: { single?: number; offence?: number; defence?: number };
  nodes: Record<string, { single?: number; offence?: number; defence?: number; path?: number }>;
};

export type Gem = {
  name: string;
  level: number;
  quality: number;
  qualityId: string;
  enabled: boolean;
  color?: string;
  error?: string;
  support: boolean;
  count?: number;
};

export type SocketGroup = {
  label?: string;
  displayLabel?: string;
  slot?: string;
  enabled: boolean;
  includeInFullDPS: boolean;
  source?: string;
  gems: Gem[];
};

export type Skills = {
  groups: SocketGroup[];
  mainSocketGroup: number;
  slots: string[];
  skillSets: { id: number; title: string }[];
  activeSkillSet: number;
};

export type Item = { id: number; name: string; title?: string; baseName?: string; rarity: string; type: string; raw: string };

export type ItemSlot = { name: string; label: string; shown: boolean; item?: number; nodeId?: number; candidates: number[] };

export type Items = {
  items: Item[];
  slots: ItemSlot[];
  itemSets: { id: number; title: string }[];
  activeItemSet: number;
};

export type ConfigVar = Control & { var: string; value: unknown };

export type Config = {
  sections: { label: string; col?: number; vars: ConfigVar[] }[];
  configSets: { id: number; title: string }[];
  activeConfigSet: number;
};

export type CalcCell = { text?: string; breakdown?: number[]; control?: string };

export type Calcs = {
  sections: {
    id: string;
    group: number;
    color: string;
    subs: { label: string; extra?: string; collapsed?: boolean; rows: { label?: string; color?: string; cells: CalcCell[] }[] }[];
  }[];
  showMinion: boolean;
};

export type Breakdown = {
  sections: (
    | { type: "text"; label?: string; lines: string[] }
    | { type: "table"; label?: string; footer?: string; cols: { key: string; label: string }[]; rows: Record<string, string>[] }
  )[];
};

export type DisplayItem = { item?: Item; existing?: boolean; lines?: TooltipLine[]; controls?: Control[] };
