// What the engine does not have yet, where a view or control would be:
// an engine built without the translation answers commands the typed
// engine does not have with "not ported"
export function NotPorted({ what }: { what: string }) {
  return <p class="not-ported">Not available in this engine yet: {what}.</p>;
}

// A tooltip for a control whose command is not available
export const notPortedTitle = "Not available in this engine yet";
