import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const NAMED_COLORS = new Set(
  (
    "aliceblue antiquewhite aqua aquamarine azure beige bisque black blanchedalmond blue blueviolet brown burlywood " +
    "cadetblue chartreuse chocolate coral cornflowerblue cornsilk crimson cyan darkblue darkcyan darkgoldenrod darkgray " +
    "darkgreen darkgrey darkkhaki darkmagenta darkolivegreen darkorange darkorchid darkred darksalmon darkseagreen " +
    "darkslateblue darkslategray darkslategrey darkturquoise darkviolet deeppink deepskyblue dimgray dimgrey dodgerblue " +
    "firebrick floralwhite forestgreen fuchsia gainsboro ghostwhite gold goldenrod gray green greenyellow grey honeydew " +
    "hotpink indianred indigo ivory khaki lavender lavenderblush lawngreen lemonchiffon lightblue lightcoral lightcyan " +
    "lightgoldenrodyellow lightgray lightgreen lightgrey lightpink lightsalmon lightseagreen lightskyblue lightslategray " +
    "lightslategrey lightsteelblue lightyellow lime limegreen linen magenta maroon mediumaquamarine mediumblue " +
    "mediumorchid mediumpurple mediumseagreen mediumslateblue mediumspringgreen mediumturquoise mediumvioletred " +
    "midnightblue mintcream mistyrose moccasin navajowhite navy oldlace olive olivedrab orange orangered orchid " +
    "palegoldenrod palegreen paleturquoise palevioletred papayawhip peachpuff peru pink plum powderblue purple " +
    "rebeccapurple red rosybrown royalblue saddlebrown salmon sandybrown seagreen seashell sienna silver skyblue " +
    "slateblue slategray slategrey snow springgreen steelblue tan teal thistle tomato turquoise violet wheat white " +
    "whitesmoke yellow yellowgreen"
  ).split(" "),
);

const COLOR_PROPERTY =
  /(?:^|[;{\s"'`])(?:color|background(?:-color)?|border(?:-(?:top|right|bottom|left))?(?:-color)?|outline(?:-color)?|fill|stroke|caret-color|accent-color|box-shadow|text-shadow)\s*:\s*([^;}"'`]*)/gi;
const HEX = /(?<![&\w"'`/&-])#(?:[0-9a-f]{8}|[0-9a-f]{6}|[0-9a-f]{4}|[0-9a-f]{3})\b/gi;
const COLOR_FUNCTION = /\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color)\(\s*(?!var\()/gi;
const SVG_COLOR_ATTR = /\b(?:fill|stroke|stop-color|flood-color)\s*=\s*"([^"{]*)"/gi;

function lineOf(text, index) {
  return text.slice(0, index).split("\n").length;
}

function isAllowedKeyword(word) {
  return ["none", "transparent", "currentcolor", "inherit", "initial", "unset", "revert"].includes(word);
}

function namedColorsIn(value) {
  const cleaned = value.replace(/var\([^)]*\)/g, " ").replace(/\burl\([^)]*\)/g, " ");
  return cleaned
    .split(/[\s,()/]+/)
    .map((word) => word.toLowerCase())
    .filter((word) => /^[a-z]+$/.test(word) && NAMED_COLORS.has(word) && !isAllowedKeyword(word));
}

/** Returns one message per literal color found in a .svelte source. */
export function lintSource(text) {
  const problems = [];
  const stripped = text.replace(/<!--[\s\S]*?-->/g, (match) => match.replace(/[^\n]/g, " "));
  for (const match of stripped.matchAll(HEX)) {
    problems.push({ line: lineOf(stripped, match.index), message: `literal color ${match[0]}` });
  }
  for (const match of stripped.matchAll(COLOR_FUNCTION)) {
    problems.push({ line: lineOf(stripped, match.index), message: `literal color function ${match[0].trim()}` });
  }
  for (const match of stripped.matchAll(COLOR_PROPERTY)) {
    for (const word of namedColorsIn(match[1] ?? "")) {
      problems.push({ line: lineOf(stripped, match.index), message: `named color "${word}"` });
    }
  }
  for (const match of stripped.matchAll(SVG_COLOR_ATTR)) {
    const value = (match[1] ?? "").trim().toLowerCase();
    if (value !== "" && !isAllowedKeyword(value) && !value.startsWith("var(")) {
      problems.push({ line: lineOf(stripped, match.index), message: `literal svg color "${match[1]}"` });
    }
  }
  return problems;
}

function* walk(directory) {
  for (const name of readdirSync(directory)) {
    const path = join(directory, name);
    if (statSync(path).isDirectory()) yield* walk(path);
    else if (path.endsWith(".svelte")) yield path;
  }
}

export function lintTree(root) {
  const failures = [];
  for (const file of walk(root)) {
    for (const problem of lintSource(readFileSync(file, "utf8"))) {
      failures.push({ file, ...problem });
    }
  }
  return failures;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = resolve(fileURLToPath(new URL("../src", import.meta.url)));
  const failures = lintTree(root);
  for (const failure of failures) {
    console.error(`${relative(process.cwd(), failure.file)}:${failure.line}: ${failure.message}`);
  }
  if (failures.length > 0) {
    console.error(`lint:tokens failed: ${failures.length} literal color(s) in .svelte files. Use var(--...) tokens.`);
    process.exit(1);
  }
  console.log("lint:tokens ok: no literal colors in .svelte files");
}
