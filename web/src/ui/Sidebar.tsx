import type { TabProps } from "../app";
import { ColorText } from "./ColorText";
import { ControlView } from "./Control";

const order: [string, string?][] = [
  ["build.characterLevel", "Level"],
  ["build.classDrop", "Class"],
  ["build.ascendDrop", "Ascendancy"],
  ["build.secondaryAscendDrop", "Bloodline"],
  ["build.mainSocketGroup", "Main skill"],
  ["build.mainSkill"],
  ["build.mainSkillPart", "Skill part"],
  ["build.mainSkillStageCount", "Stages"],
  ["build.mainSkillMineCount", "Active mines"],
  ["build.mainSkillMinion", "Minion"],
  ["build.mainSkillMinionSkill", "Minion skill"],
];

export function Sidebar({ state, setControl }: TabProps) {
  return (
    <div>
      <div class="points">
        <ColorText text={state.pointsText ?? `${state.points.used} points`} />
      </div>
      <div class="sidebar-controls">
        {order.map(([path, label]) => (
          <ControlView key={path} path={path} c={state.controls[path]} set={setControl} label={label} />
        ))}
      </div>
      <table class="stats">
        <tbody>
          {state.stats.map((row, i) =>
            row.gap ? (
              <tr key={i} class="gap">
                <td colSpan={2} />
              </tr>
            ) : row.center || row[1] === undefined ? (
              <tr key={i}>
                <td colSpan={2} class="center">
                  <ColorText text={row[0]} />
                </td>
              </tr>
            ) : (
              <tr key={i}>
                <td class="stat-label">
                  <ColorText text={row[0]} />
                </td>
                <td class="stat-value">
                  <ColorText text={row[1]} />
                </td>
              </tr>
            ),
          )}
        </tbody>
      </table>
      {state.warnings.length ? (
        <ul class="warnings">
          {state.warnings.map((w, i) => (
            <li key={i}>
              <ColorText text={w} />
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
