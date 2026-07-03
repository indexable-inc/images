// The dashboard's shiki themes, generated from the JetBrains Islands palette
// owned by packages/code/code-highlight/src/islands-theme.json (resolved
// through the `$islands-theme` vite alias). Only the slot → TextMate-scope
// mapping lives here; edit colors in the palette file, never here.
import palette from '$islands-theme';

type Palette = Record<string, string>;

// Which palette slot colors which TextMate scopes. TextMate resolution picks
// the most specific matching selector, so broad rules (comment, string,
// keyword) coexist with their refinements (doc_comment, string_escape).
const TOKEN_SCOPES: [slot: string, scopes: string[]][] = [
  ['comment', ['comment', 'punctuation.definition.comment']],
  ['doc_comment', ['comment.block.documentation', 'comment.line.documentation']],
  ['string', ['string', 'punctuation.definition.string']],
  ['string_escape', ['constant.character.escape']],
  ['number', ['constant.numeric']],
  ['keyword', ['keyword', 'keyword.control', 'storage', 'storage.type', 'storage.modifier']],
  ['operator', ['keyword.operator']],
  ['func', ['entity.name.function', 'support.function', 'meta.function-call.generic']],
  ['type', ['entity.name.type', 'entity.name.class', 'support.type', 'support.class']],
  ['type_param', ['entity.name.type.parameter']],
  ['variable', ['variable']],
  ['parameter', ['variable.parameter']],
  ['constant', ['constant.language', 'variable.other.constant', 'support.constant']],
  [
    'property',
    [
      'variable.other.property',
      'variable.other.object.property',
      'support.type.property-name',
      'entity.other.attribute-name',
    ],
  ],
  ['this_self', ['variable.language']],
  ['punctuation', ['punctuation']],
  ['tag', ['entity.name.tag']],
  ['decorator', ['meta.decorator', 'entity.name.function.decorator', 'punctuation.decorator']],
  ['regexp', ['string.regexp']],
  ['shell_var', ['variable.other.normal', 'punctuation.definition.variable']],
  ['link', ['markup.underline.link']],
  ['heading', ['markup.heading', 'entity.name.section']],
  ['invalid', ['invalid.illegal']],
];

// A shiki ThemeRegistrationRaw (a raw TextMate theme object, accepted inline by
// codeToHtml's `themes` option without registration).
export interface IslandsTheme {
  name: string;
  type: 'light' | 'dark';
  colors: Record<string, string>;
  settings: { scope?: string[]; settings: Record<string, string> }[];
}

function theme(name: string, type: 'light' | 'dark', p: Palette): IslandsTheme {
  return {
    name,
    type,
    colors: {
      'editor.background': p.bg,
      'editor.foreground': p.fg,
    },
    settings: [
      // The scopeless first rule is the TextMate global default (fg/bg).
      { settings: { foreground: p.fg, background: p.bg } },
      ...TOKEN_SCOPES.filter(([slot]) => p[slot]).map(([slot, scope]) => ({
        scope,
        settings: { foreground: p[slot] },
      })),
    ],
  };
}

export const islandsDark = theme('islands-dark', 'dark', palette.dark);
export const islandsLight = theme('islands-light', 'light', palette.light);
