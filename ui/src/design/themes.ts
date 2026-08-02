/**
 * Tervin themes.
 *
 * A theme carries two things that must stay consistent with each other:
 *
 * 1. **Surface tokens** — the workspace chrome. These follow the brand system:
 *    graphite and off-white dominate, teal marks focus and intentional action,
 *    and green/amber/red are semantic state colours only. No gradients, no glow.
 *
 * 2. **A full 16-colour ANSI palette** — what the terminal actually renders with.
 *    This is not decoration. Prompt frameworks such as oh-my-zsh, powerlevel10k,
 *    starship, and spaceship draw themselves out of the ANSI palette, so a theme
 *    that only styled the chrome would leave every prompt looking wrong. Both
 *    halves ship together for that reason.
 *
 * Fifteen themes, deliberately capped. Beyond that the list stops being a choice
 * and becomes a scrolling problem, and each one is a surface Tervin has to keep
 * legible at every contrast level.
 */

/** The 16 standard ANSI colours a terminal renders text with. */
export interface AnsiPalette {
  black: string;
  red: string;
  green: string;
  yellow: string;
  blue: string;
  magenta: string;
  cyan: string;
  white: string;
  brightBlack: string;
  brightRed: string;
  brightGreen: string;
  brightYellow: string;
  brightBlue: string;
  brightMagenta: string;
  brightCyan: string;
  brightWhite: string;
}

/** Workspace chrome colours. Names match the brand tokens in the spec. */
export interface SurfaceTokens {
  /** Main app background. */
  bg: string;
  /** Panels and terminal surfaces. */
  panel: string;
  /** Raised surfaces: popovers, the palette, approval sheets. */
  raised: string;
  /** Dividers and subtle boundaries. */
  line: string;
  /** Primary text. */
  ink: string;
  /** Secondary information. */
  muted: string;
  /** Focus, primary action, and the brand seam. */
  accent: string;
  /** Passing command or test state. */
  green: string;
  /** Warning, plan mode, pending review. */
  amber: string;
  /** Failure and destructive-action warnings. */
  red: string;
  /** Terminal background. Usually equal to `panel`. */
  terminalBg: string;
  /** Terminal foreground. */
  terminalFg: string;
  /** Cursor colour. */
  cursor: string;
  /** Selection background. Must keep text readable underneath. */
  selection: string;

  /*
   * The tokens below are optional. The design system names all of them, but only
   * the default theme pins exact values — for the other fourteen they are derived
   * from the four anchors above via `color-mix`, which keeps each theme a short,
   * reviewable list instead of fourteen near-identical blocks that drift apart.
   */

  /** Terminal output and secondary body text. Between `ink` and `muted`. */
  ink2?: string;
  /** Timestamps, keyboard hints, placeholder text. */
  dim?: string;
  /** Hover state on the accent. */
  accentHi?: string;
  /** Overlay surface: palette, modals, menus. */
  overlay?: string;
  /** Borders inside lists and panels, as opposed to between regions. */
  hairline?: string;
  /** Block row hover background. One step above the panel. */
  blockHover?: string;
}

export interface Theme {
  id: string;
  name: string;
  /** Drives `prefers-color-scheme` behaviour and the terminal's own assumptions. */
  appearance: "dark" | "light";
  /** One line on what this theme is for. */
  note: string;
  surface: SurfaceTokens;
  ansi: AnsiPalette;
}

/**
 * The Tervin palette from the brand system, used as the default theme's base.
 * Kept as named constants so the brand values appear exactly once.
 */
export const BRAND = {
  graphite950: "#141514",
  graphite900: "#1B1D1C",
  graphite800: "#232624",
  line: "#323634",
  ink: "#E5E8E5",
  /** Terminal output and secondary body text. */
  ink2: "#AEB5B1",
  muted: "#909894",
  /** Timestamps, keyboard hints, placeholders. */
  dim: "#5d635f",
  teal: "#68AEA5",
  tealBright: "#8CC9C1",
  /** Overlay surface. */
  overlay: "#171918",
  /** Block row hover. */
  blockHover: "#181A19",
  green: "#85BC7E",
  amber: "#D5AB68",
  red: "#D77D79",
} as const;

/**
 * Build a theme with less repetition. Surface and ANSI values are still explicit
 * per theme — a derived palette looks synthetic, and terminal colours in
 * particular need hand-picking to stay distinguishable.
 */
function theme(
  id: string,
  name: string,
  appearance: "dark" | "light",
  note: string,
  surface: SurfaceTokens,
  ansi: AnsiPalette,
): Theme {
  return { id, name, appearance, note, surface, ansi };
}

export const THEMES: Theme[] = [
  theme(
    "tervin-dark",
    "Tervin Dark",
    "dark",
    "The default. Warm graphite, restrained teal focus.",
    {
      bg: BRAND.graphite950,
      panel: BRAND.graphite900,
      raised: BRAND.graphite800,
      line: BRAND.line,
      ink: BRAND.ink,
      muted: BRAND.muted,
      accent: BRAND.teal,
      green: BRAND.green,
      amber: BRAND.amber,
      red: BRAND.red,
      terminalBg: BRAND.graphite900,
      terminalFg: BRAND.ink,
      cursor: BRAND.teal,
      selection: "#2F4B47",
      // Exact values from the design system, not derived.
      ink2: BRAND.ink2,
      dim: BRAND.dim,
      accentHi: BRAND.tealBright,
      overlay: BRAND.overlay,
      hairline: BRAND.graphite900,
      blockHover: BRAND.blockHover,
    },
    {
      black: "#232624",
      red: "#D77D79",
      green: "#85BC7E",
      yellow: "#D5AB68",
      blue: "#7FA6C4",
      magenta: "#B195C0",
      cyan: "#68AEA5",
      white: "#C9CFCB",
      brightBlack: "#6B736F",
      brightRed: "#E89A96",
      brightGreen: "#A2D29B",
      brightYellow: "#E6C48A",
      brightBlue: "#9DBFD8",
      brightMagenta: "#C9B0D4",
      brightCyan: "#8CC9C1",
      brightWhite: "#E5E8E5",
    },
  ),

  theme(
    "tervin-light",
    "Tervin Light",
    "light",
    "The brand palette inverted for bright rooms.",
    {
      bg: "#F7F8F7",
      panel: "#FFFFFF",
      raised: "#EFF1EF",
      line: "#D8DCD9",
      ink: "#1B1D1C",
      muted: "#5F6764",
      accent: "#3D8880",
      green: "#4B8A45",
      amber: "#996716",
      red: "#B04B47",
      terminalBg: "#FFFFFF",
      terminalFg: "#1B1D1C",
      cursor: "#3D8880",
      selection: "#CFE3E0",
      ink2: "#3C4441",
      dim: "#8B928F",
      accentHi: "#2C6F67",
      overlay: "#FFFFFF",
      hairline: "#E4E7E5",
      blockHover: "#F1F3F1",
    },
    {
      black: "#1B1D1C",
      red: "#B04B47",
      green: "#4B8A45",
      yellow: "#996716",
      blue: "#3A6E96",
      magenta: "#7A5590",
      cyan: "#3D8880",
      white: "#5F6764",
      brightBlack: "#8B928F",
      brightRed: "#C96662",
      brightGreen: "#5FA357",
      brightYellow: "#B4801F",
      brightBlue: "#4E86B0",
      brightMagenta: "#9270A8",
      brightCyan: "#4E9C93",
      brightWhite: "#232624",
    },
  ),

  theme(
    "tervin-contrast",
    "Tervin High Contrast",
    "dark",
    "Maximum legibility. Meets WCAG AAA for body text.",
    {
      bg: "#000000",
      panel: "#0A0B0A",
      raised: "#161816",
      line: "#4A514D",
      ink: "#FFFFFF",
      muted: "#B9C0BC",
      accent: "#7FD9CE",
      green: "#8FE585",
      amber: "#F2C87A",
      red: "#FF9B96",
      terminalBg: "#000000",
      terminalFg: "#FFFFFF",
      cursor: "#7FD9CE",
      selection: "#31544F",
    },
    {
      black: "#1A1C1A",
      red: "#FF9B96",
      green: "#8FE585",
      yellow: "#F2C87A",
      blue: "#9EC8E8",
      magenta: "#D4B4E2",
      cyan: "#7FD9CE",
      white: "#E8ECE9",
      brightBlack: "#8A918D",
      brightRed: "#FFBDB9",
      brightGreen: "#B0F0A8",
      brightYellow: "#FFDFA6",
      brightBlue: "#C0DDF2",
      brightMagenta: "#E6CFEF",
      brightCyan: "#A6E8E0",
      brightWhite: "#FFFFFF",
    },
  ),

  theme(
    "graphite-mono",
    "Graphite Mono",
    "dark",
    "Near-monochrome. Colour reserved entirely for state.",
    {
      bg: "#121312",
      panel: "#191A19",
      raised: "#212322",
      line: "#303331",
      ink: "#E3E5E3",
      muted: "#8B908D",
      accent: "#B9BFBB",
      green: "#8FB98A",
      amber: "#C7A874",
      red: "#C88580",
      terminalBg: "#191A19",
      terminalFg: "#E3E5E3",
      cursor: "#E3E5E3",
      selection: "#33403D",
    },
    {
      black: "#212322",
      red: "#C88580",
      green: "#8FB98A",
      yellow: "#C7A874",
      blue: "#93A6B4",
      magenta: "#A99BB2",
      cyan: "#8FB0AC",
      white: "#C2C6C4",
      brightBlack: "#6E736F",
      brightRed: "#DBA19C",
      brightGreen: "#A9CCA4",
      brightYellow: "#DBC194",
      brightBlue: "#AFC0CB",
      brightMagenta: "#C1B6C8",
      brightCyan: "#A9C7C3",
      brightWhite: "#F0F2F0",
    },
  ),

  theme(
    "slate",
    "Slate",
    "dark",
    "Cool neutral grey with a steady blue accent.",
    {
      bg: "#101317",
      panel: "#161A20",
      raised: "#1E242B",
      line: "#2C333C",
      ink: "#DDE3EA",
      muted: "#8894A2",
      accent: "#6FA8C7",
      green: "#7FB77E",
      amber: "#CDA96B",
      red: "#CE7F7F",
      terminalBg: "#161A20",
      terminalFg: "#DDE3EA",
      cursor: "#6FA8C7",
      selection: "#2B4152",
    },
    {
      black: "#1E242B",
      red: "#CE7F7F",
      green: "#7FB77E",
      yellow: "#CDA96B",
      blue: "#6FA8C7",
      magenta: "#A78FC0",
      cyan: "#6FBAB5",
      white: "#C2CAD3",
      brightBlack: "#68747F",
      brightRed: "#E29B9B",
      brightGreen: "#9BCE9A",
      brightYellow: "#E0C28C",
      brightBlue: "#93C2DA",
      brightMagenta: "#C0ACD3",
      brightCyan: "#92D0CB",
      brightWhite: "#EDF1F5",
    },
  ),

  theme(
    "ember",
    "Ember",
    "dark",
    "Warm brown-black with a copper accent.",
    {
      bg: "#16120F",
      panel: "#1D1916",
      raised: "#26211D",
      line: "#37302A",
      ink: "#E9E1D8",
      muted: "#9A8F84",
      accent: "#C98F5E",
      green: "#9BB472",
      amber: "#D9AE6A",
      red: "#CE7E6E",
      terminalBg: "#1D1916",
      terminalFg: "#E9E1D8",
      cursor: "#C98F5E",
      selection: "#4A3A2B",
    },
    {
      black: "#26211D",
      red: "#CE7E6E",
      green: "#9BB472",
      yellow: "#D9AE6A",
      blue: "#8FA6B8",
      magenta: "#B893A5",
      cyan: "#7EB2A8",
      white: "#CFC5B9",
      brightBlack: "#75695F",
      brightRed: "#E09B8D",
      brightGreen: "#B5CB90",
      brightYellow: "#EBC98C",
      brightBlue: "#AAC0CE",
      brightMagenta: "#D0AFBE",
      brightCyan: "#9CCAC0",
      brightWhite: "#F5EFE7",
    },
  ),

  theme(
    "moss",
    "Moss",
    "dark",
    "Deep green-grey, easy for long sessions.",
    {
      bg: "#101410",
      panel: "#161B16",
      raised: "#1E241E",
      line: "#2D352D",
      ink: "#DEE5DD",
      muted: "#8A968A",
      accent: "#79B58E",
      green: "#8CC189",
      amber: "#CFAE72",
      red: "#CC8380",
      terminalBg: "#161B16",
      terminalFg: "#DEE5DD",
      cursor: "#79B58E",
      selection: "#2F4636",
    },
    {
      black: "#1E241E",
      red: "#CC8380",
      green: "#8CC189",
      yellow: "#CFAE72",
      blue: "#87A9BC",
      magenta: "#AE96BE",
      cyan: "#79B5A8",
      white: "#C4CCC3",
      brightBlack: "#6B776B",
      brightRed: "#DFA09D",
      brightGreen: "#A8D6A5",
      brightYellow: "#E2C793",
      brightBlue: "#A5C3D3",
      brightMagenta: "#C7B3D3",
      brightCyan: "#98CDC2",
      brightWhite: "#EEF3ED",
    },
  ),

  theme(
    "ink-blue",
    "Ink Blue",
    "dark",
    "Navy surfaces with a cool cyan seam.",
    {
      bg: "#0D1219",
      panel: "#121824",
      raised: "#1A2130",
      line: "#27303F",
      ink: "#D8E1EC",
      muted: "#7F8DA0",
      accent: "#5FB3C4",
      green: "#79B98C",
      amber: "#CBA96E",
      red: "#CC7F84",
      terminalBg: "#121824",
      terminalFg: "#D8E1EC",
      cursor: "#5FB3C4",
      selection: "#26445A",
    },
    {
      black: "#1A2130",
      red: "#CC7F84",
      green: "#79B98C",
      yellow: "#CBA96E",
      blue: "#6E9FD4",
      magenta: "#A48EC9",
      cyan: "#5FB3C4",
      white: "#BCC6D3",
      brightBlack: "#5F6C7E",
      brightRed: "#E09BA0",
      brightGreen: "#96D0A8",
      brightYellow: "#DFC28E",
      brightBlue: "#92BAE4",
      brightMagenta: "#BFACDD",
      brightCyan: "#85CBDA",
      brightWhite: "#EAF0F7",
    },
  ),

  theme(
    "paper",
    "Paper",
    "light",
    "Warm off-white, low glare.",
    {
      bg: "#F6F4EF",
      panel: "#FDFCF8",
      raised: "#EEEBE3",
      line: "#DAD5C9",
      ink: "#232019",
      muted: "#67614F",
      accent: "#3F7F76",
      green: "#4B7F3E",
      amber: "#8C6415",
      red: "#A94B42",
      terminalBg: "#FDFCF8",
      terminalFg: "#232019",
      cursor: "#3F7F76",
      selection: "#D9E5E1",
    },
    {
      black: "#232019",
      red: "#A94B42",
      green: "#4B7F3E",
      yellow: "#8C6415",
      blue: "#356A93",
      magenta: "#77518A",
      cyan: "#3F7F76",
      white: "#67614F",
      brightBlack: "#918A76",
      brightRed: "#C2645A",
      brightGreen: "#619B52",
      brightYellow: "#A87B22",
      brightBlue: "#4A82AD",
      brightMagenta: "#8F6BA3",
      brightCyan: "#4F978D",
      brightWhite: "#2E2A21",
    },
  ),

  theme(
    "porcelain",
    "Porcelain",
    "light",
    "Cool, crisp light theme for high ambient light.",
    {
      bg: "#F4F6F8",
      panel: "#FFFFFF",
      raised: "#E9EDF1",
      line: "#D3DAE1",
      ink: "#141A20",
      muted: "#5A646E",
      accent: "#2F7A94",
      green: "#437F49",
      amber: "#8A6412",
      red: "#AB4A4F",
      terminalBg: "#FFFFFF",
      terminalFg: "#141A20",
      cursor: "#2F7A94",
      selection: "#CFE2EA",
    },
    {
      black: "#141A20",
      red: "#AB4A4F",
      green: "#437F49",
      yellow: "#8A6412",
      blue: "#2C6494",
      magenta: "#71518F",
      cyan: "#2F7A94",
      white: "#5A646E",
      brightBlack: "#8A939C",
      brightRed: "#C56469",
      brightGreen: "#589A5E",
      brightYellow: "#A67B1E",
      brightBlue: "#3E7CB0",
      brightMagenta: "#8B6BA9",
      brightCyan: "#3F93AF",
      brightWhite: "#1E262E",
    },
  ),

  theme(
    "nocturne",
    "Nocturne",
    "dark",
    "Very dark, low-luminance. Built for night work.",
    {
      bg: "#08090A",
      panel: "#0D0F10",
      raised: "#14171A",
      line: "#22272B",
      ink: "#C8CFD4",
      muted: "#727C83",
      accent: "#4E9A96",
      green: "#6FA26C",
      amber: "#B4915C",
      red: "#B26C6C",
      terminalBg: "#0D0F10",
      terminalFg: "#C8CFD4",
      cursor: "#4E9A96",
      selection: "#22403E",
    },
    {
      black: "#14171A",
      red: "#B26C6C",
      green: "#6FA26C",
      yellow: "#B4915C",
      blue: "#6D8CA6",
      magenta: "#8F7DA3",
      cyan: "#4E9A96",
      white: "#A6ADB3",
      brightBlack: "#555E64",
      brightRed: "#C88686",
      brightGreen: "#89BB86",
      brightYellow: "#CBAA75",
      brightBlue: "#88A6BF",
      brightMagenta: "#A996BC",
      brightCyan: "#68B3AE",
      brightWhite: "#DDE3E7",
    },
  ),

  theme(
    "sandstone",
    "Sandstone",
    "light",
    "Muted sand tones. Warmer than Paper, still low contrast fatigue.",
    {
      bg: "#F2EEE7",
      panel: "#FAF7F2",
      raised: "#E8E2D8",
      line: "#D3CABB",
      ink: "#2A251E",
      muted: "#6B6252",
      accent: "#457C6F",
      green: "#4F7C3C",
      amber: "#8A6113",
      red: "#A54B41",
      terminalBg: "#FAF7F2",
      terminalFg: "#2A251E",
      cursor: "#457C6F",
      selection: "#DCE6E0",
    },
    {
      black: "#2A251E",
      red: "#A54B41",
      green: "#4F7C3C",
      yellow: "#8A6113",
      blue: "#3A6788",
      magenta: "#755084",
      cyan: "#457C6F",
      white: "#6B6252",
      brightBlack: "#948A79",
      brightRed: "#BE6459",
      brightGreen: "#679A51",
      brightYellow: "#A67A21",
      brightBlue: "#4F7EA2",
      brightMagenta: "#8D6A9C",
      brightCyan: "#569488",
      brightWhite: "#352F26",
    },
  ),

  theme(
    "carbon",
    "Carbon",
    "dark",
    "Flat neutral dark with sharp separation between panes.",
    {
      bg: "#0F0F10",
      panel: "#151517",
      raised: "#1D1D20",
      line: "#2B2B2F",
      ink: "#DEDEE1",
      muted: "#88888F",
      accent: "#6BA8B8",
      green: "#82B87C",
      amber: "#CBA96A",
      red: "#CB8080",
      terminalBg: "#151517",
      terminalFg: "#DEDEE1",
      cursor: "#6BA8B8",
      selection: "#2E4149",
    },
    {
      black: "#1D1D20",
      red: "#CB8080",
      green: "#82B87C",
      yellow: "#CBA96A",
      blue: "#7C9FCB",
      magenta: "#A992C4",
      cyan: "#6BA8B8",
      white: "#C0C0C6",
      brightBlack: "#6A6A72",
      brightRed: "#DE9C9C",
      brightGreen: "#9FCF99",
      brightYellow: "#DFC28B",
      brightBlue: "#9CBADF",
      brightMagenta: "#C4B0D8",
      brightCyan: "#8CC2D0",
      brightWhite: "#EFEFF2",
    },
  ),

  theme(
    "solar-dim",
    "Solar Dim",
    "dark",
    "Muted amber-on-dark, easy on tired eyes.",
    {
      bg: "#14130F",
      panel: "#1A1914",
      raised: "#23211B",
      line: "#333026",
      ink: "#E4DFD1",
      muted: "#948F7E",
      accent: "#B8A263",
      green: "#93B172",
      amber: "#D2B36B",
      red: "#C58275",
      terminalBg: "#1A1914",
      terminalFg: "#E4DFD1",
      cursor: "#B8A263",
      selection: "#41402F",
    },
    {
      black: "#23211B",
      red: "#C58275",
      green: "#93B172",
      yellow: "#D2B36B",
      blue: "#8AA3AE",
      magenta: "#AE96A8",
      cyan: "#84AFA1",
      white: "#C8C2B2",
      brightBlack: "#726D5F",
      brightRed: "#D89D91",
      brightGreen: "#AFC98F",
      brightYellow: "#E5CB8C",
      brightBlue: "#A6BCC5",
      brightMagenta: "#C7B2C1",
      brightCyan: "#A2C7BA",
      brightWhite: "#F1EDE1",
    },
  ),

  theme(
    "monochrome",
    "Monochrome",
    "dark",
    "No hue at all. State is shown by weight and marker, not colour.",
    {
      bg: "#0E0E0E",
      panel: "#141414",
      raised: "#1C1C1C",
      line: "#2E2E2E",
      ink: "#E6E6E6",
      muted: "#8A8A8A",
      accent: "#C4C4C4",
      green: "#B4B4B4",
      amber: "#D0D0D0",
      red: "#F0F0F0",
      terminalBg: "#141414",
      terminalFg: "#E6E6E6",
      cursor: "#E6E6E6",
      selection: "#3A3A3A",
    },
    {
      black: "#1C1C1C",
      red: "#9E9E9E",
      green: "#B4B4B4",
      yellow: "#C6C6C6",
      blue: "#A8A8A8",
      magenta: "#B0B0B0",
      cyan: "#BCBCBC",
      white: "#D4D4D4",
      brightBlack: "#6E6E6E",
      brightRed: "#C0C0C0",
      brightGreen: "#CECECE",
      brightYellow: "#DCDCDC",
      brightBlue: "#C4C4C4",
      brightMagenta: "#CACACA",
      brightCyan: "#D2D2D2",
      brightWhite: "#F4F4F4",
    },
  ),
];

/** The theme applied on first run. */
export const DEFAULT_THEME_ID = "tervin-dark";

export function findTheme(id: string): Theme {
  return THEMES.find((t) => t.id === id) ?? THEMES[0]!;
}

/**
 * Apply a theme by writing CSS custom properties onto the document root.
 *
 * Done as variables rather than by swapping a stylesheet so a theme change is a
 * single style recalculation with no reflow and no flash — which is also what
 * makes live preview in the settings pane feel instant.
 */
export function applyTheme(t: Theme, root: HTMLElement = document.documentElement): void {
  const s = t.surface;
  const set = (k: string, v: string) => root.style.setProperty(k, v);

  set("--tervin-bg", s.bg);
  set("--tervin-panel", s.panel);
  set("--tervin-raised", s.raised);
  set("--tervin-line", s.line);
  set("--tervin-ink", s.ink);
  set("--tervin-muted", s.muted);
  set("--tervin-accent", s.accent);
  set("--tervin-green", s.green);
  set("--tervin-amber", s.amber);
  set("--tervin-red", s.red);
  // Derived where a theme did not pin them. `color-mix` keeps the relationship
  // between tokens intact across light and dark themes, which a fixed offset
  // would not.
  set("--tervin-ink-2", s.ink2 ?? `color-mix(in srgb, ${s.ink} 68%, ${s.muted})`);
  set("--tervin-dim", s.dim ?? `color-mix(in srgb, ${s.muted} 64%, ${s.panel})`);
  set(
    "--tervin-accent-hi",
    s.accentHi ??
      `color-mix(in srgb, ${s.accent} 76%, ${t.appearance === "dark" ? "white" : "black"})`,
  );
  set("--tervin-overlay", s.overlay ?? `color-mix(in srgb, ${s.panel} 70%, ${s.raised})`);
  set("--tervin-hairline", s.hairline ?? s.panel);
  set(
    "--tervin-block-hover",
    s.blockHover ?? `color-mix(in srgb, ${s.panel} 92%, ${s.ink})`,
  );

  set("--tervin-terminal-bg", s.terminalBg);
  set("--tervin-terminal-fg", s.terminalFg);
  set("--tervin-cursor", s.cursor);
  set("--tervin-selection", s.selection);

  root.dataset.appearance = t.appearance;
  root.style.colorScheme = t.appearance;
}

/** Convert a theme's ANSI palette into the shape xterm.js expects. */
export function toXtermTheme(t: Theme): Record<string, string> {
  return {
    background: t.surface.terminalBg,
    foreground: t.surface.terminalFg,
    cursor: t.surface.cursor,
    cursorAccent: t.surface.terminalBg,
    selectionBackground: t.surface.selection,
    // xterm computes selection foreground itself when omitted, which keeps text
    // readable across themes better than a fixed value would.
    black: t.ansi.black,
    red: t.ansi.red,
    green: t.ansi.green,
    yellow: t.ansi.yellow,
    blue: t.ansi.blue,
    magenta: t.ansi.magenta,
    cyan: t.ansi.cyan,
    white: t.ansi.white,
    brightBlack: t.ansi.brightBlack,
    brightRed: t.ansi.brightRed,
    brightGreen: t.ansi.brightGreen,
    brightYellow: t.ansi.brightYellow,
    brightBlue: t.ansi.brightBlue,
    brightMagenta: t.ansi.brightMagenta,
    brightCyan: t.ansi.brightCyan,
    brightWhite: t.ansi.brightWhite,
  };
}
