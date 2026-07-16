const ANSI_PATTERN = new RegExp(
  [
    '[\\u001B\\u009B][[\\]()#;?]*(?:(?:(?:(?:;[-a-zA-Z\\d/#&.:=?%@~_]+)*|[a-zA-Z\\d]+(?:;[-a-zA-Z\\d/#&.:=?%@~_]*)*)?\\u0007)',
    '(?:(?:\\d{1,4}(?:;\\d{0,4})*)?[\\dA-PR-TZcf-ntqry=><~]))',
  ].join('|'),
  'g',
);

/** Strips ANSI escape/control sequences from raw PTY output for plain-text display. */
export function stripAnsi(text: string): string {
  return text.replace(ANSI_PATTERN, '');
}

/** Finds the first http(s) URL in a block of text, if any. */
export function extractUrl(text: string): string | null {
  const match = text.match(/https?:\/\/\S+/);
  return match ? match[0].replace(/[).,]+$/, '') : null;
}
