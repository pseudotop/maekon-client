/**
 * #13737: screens must not render a raw error message.
 *
 * Backend diagnostics are English by rule (ADR-019), and the frontend's own
 * `throw new Error('Tag query failed')` literals are English too. Rendering
 * `error.message` into a toast or an alert therefore puts an English sentence,
 * or an internal code, in front of a Korean user. Screens go through
 * `useErrorDisplay` / `describeErrorForDisplay` (`src/i18n/errorDisplay.ts`)
 * instead: a localized sentence for the surface, plus the raw text as collapsed
 * support detail.
 *
 * The scan is syntactic, so it is cheap and needs no type checker. It flags:
 * - `.message` read from an error-shaped expression — a name such as `error`,
 *   `err`, `e` or `*Error`, a `(x as Error)` cast, or a value narrowed by
 *   `instanceof Error` in an enclosing condition;
 * - `String(<error>)`, which renders `Error: <message>`;
 * - the raw helper `errorMessageFromInvoke`.
 *
 * A read that is never displayed (pattern matching for classification,
 * logging) carries a `raw-error-text: <reason>` comment on the same line, the
 * line above, or the enclosing statement.
 */

import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import ts from 'typescript'
import { describe, expect, it } from 'vitest'

const SRC_ROOT = join(process.cwd(), 'src')
const SCANNED_ROOTS = ['pages', 'components', 'overlay', 'hooks', 'tracking-panel']
const MARKER = 'raw-error-text:'
const ERROR_NAME = /^(?:e|err|error)$|(?:Err|Error)$/
const RAW_HELPERS = new Set(['errorMessageFromInvoke'])

/** All admitted consumer groups now use the shared display contract. */
const PENDING_RAW_DEBT: Readonly<Record<string, number>> = {}
const PENDING_DIRECT_DEBT: Readonly<Record<string, number>> = {}

interface RawErrorFinding {
  readonly kind: 'raw' | 'direct'
  readonly line: number
  readonly text: string
}

function unwrap(expression: ts.Expression): ts.Expression {
  let current = expression
  while (ts.isParenthesizedExpression(current) || ts.isNonNullExpression(current)) {
    current = current.expression
  }
  return current
}

function containsInstanceofError(node: ts.Node, name: string): boolean {
  if (
    ts.isBinaryExpression(node) &&
    node.operatorToken.kind === ts.SyntaxKind.InstanceOfKeyword &&
    ts.isIdentifier(unwrap(node.left)) &&
    (unwrap(node.left) as ts.Identifier).text === name &&
    ts.isIdentifier(node.right) &&
    node.right.text.endsWith('Error')
  ) {
    return true
  }
  return ts.forEachChild(node, (child) => containsInstanceofError(child, name) || undefined) ?? false
}

function narrowedToError(identifier: ts.Identifier): boolean {
  for (let node: ts.Node | undefined = identifier.parent; node && !ts.isFunctionLike(node); node = node.parent) {
    const condition = ts.isConditionalExpression(node)
      ? node.condition
      : ts.isIfStatement(node)
        ? node.expression
        : ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.AmpersandAmpersandToken
          ? node.left
          : undefined
    if (condition && containsInstanceofError(condition, identifier.text)) return true
  }
  return false
}

/** A discriminated error status carries diagnostics even when its name is neutral. */
function narrowedToErrorState(expression: ts.Expression, source: ts.SourceFile): boolean {
  const impliesError = (condition: ts.Expression, truth: boolean): boolean => {
    const node = unwrap(condition)
    if (ts.isPrefixUnaryExpression(node) && node.operator === ts.SyntaxKind.ExclamationToken)
      return impliesError(node.operand, !truth)
    if (!ts.isBinaryExpression(node)) return false
    const operator = node.operatorToken.kind
    if (
      (truth && operator === ts.SyntaxKind.AmpersandAmpersandToken) ||
      (!truth && operator === ts.SyntaxKind.BarBarToken)
    )
      return impliesError(node.left, truth) || impliesError(node.right, truth)
    const equality = [ts.SyntaxKind.EqualsEqualsEqualsToken, ts.SyntaxKind.EqualsEqualsToken].includes(operator)
    const inequality = [ts.SyntaxKind.ExclamationEqualsEqualsToken, ts.SyntaxKind.ExclamationEqualsToken].includes(
      operator,
    )
    if ((!equality || !truth) && (!inequality || truth)) return false
    for (const [left, right] of [
      [node.left, node.right],
      [node.right, node.left],
    ]) {
      const status = unwrap(left)
      const value = unwrap(right)
      if (
        ts.isPropertyAccessExpression(status) &&
        status.name.text === 'state' &&
        ts.isStringLiteral(value) &&
        value.text === 'error' &&
        unwrap(status.expression).getText(source) === unwrap(expression).getText(source)
      )
        return true
    }
    return false
  }
  let child: ts.Node = expression
  for (
    let node: ts.Node | undefined = child.parent;
    node && !ts.isFunctionLike(node);
    child = node, node = node.parent
  ) {
    if (ts.isConditionalExpression(node)) {
      if (child === node.whenTrue && impliesError(node.condition, true)) return true
      if (child === node.whenFalse && impliesError(node.condition, false)) return true
    } else if (ts.isIfStatement(node)) {
      if (child === node.thenStatement && impliesError(node.expression, true)) return true
      if (child === node.elseStatement && impliesError(node.expression, false)) return true
    } else if (ts.isBinaryExpression(node) && child === node.right) {
      if (node.operatorToken.kind === ts.SyntaxKind.AmpersandAmpersandToken && impliesError(node.left, true))
        return true
      if (node.operatorToken.kind === ts.SyntaxKind.BarBarToken && impliesError(node.left, false)) return true
    }
  }
  return false
}

function isErrorShaped(expression: ts.Expression, source: ts.SourceFile): boolean {
  const current = unwrap(expression)
  if (ts.isAsExpression(current) || ts.isTypeAssertionExpression(current)) {
    return /Error\b/.test(current.type.getText(source)) || isErrorShaped(current.expression, source)
  }
  const name = ts.isIdentifier(current)
    ? current.text
    : ts.isPropertyAccessExpression(current)
      ? current.name.text
      : null
  if (name !== null && ERROR_NAME.test(name)) return true
  return (ts.isIdentifier(current) && narrowedToError(current)) || narrowedToErrorState(current, source)
}

function enclosingStatement(node: ts.Node): ts.Node | undefined {
  for (let current: ts.Node | undefined = node; current; current = current.parent) {
    const parent = current.parent
    if (!parent) return undefined
    if (ts.isBlock(parent) || ts.isSourceFile(parent) || ts.isCaseClause(parent) || ts.isDefaultClause(parent)) {
      return current
    }
  }
  return undefined
}

function hasMarker(node: ts.Node, source: ts.SourceFile, lines: readonly string[]): boolean {
  const line = source.getLineAndCharacterOfPosition(node.getStart(source)).line
  if (lines[line]?.includes(MARKER) || lines[line - 1]?.includes(MARKER)) return true
  const statement = enclosingStatement(node)
  if (!statement) return false
  const text = source.getFullText()
  return (ts.getLeadingCommentRanges(text, statement.getFullStart()) ?? []).some((range) =>
    text.slice(range.pos, range.end).includes(MARKER),
  )
}

/** Resolve local import symbols without loading dependencies or a default library. */
function directHelperChecker(source: ts.SourceFile): (call: ts.CallExpression) => boolean {
  const host = ts.createCompilerHost({ noLib: true, noResolve: true })
  host.getSourceFile = (name) => (name === source.fileName ? source : undefined)
  const checker = ts.createProgram([source.fileName], { noLib: true, noResolve: true }, host).getTypeChecker()
  const modules: Readonly<Record<string, string>> = {
    translateError: 'translateError',
    describeIpcError: 'tauriIpcErrors',
  }
  const matchesImport = (declaration: ts.Node, helper: string): boolean => {
    let current: ts.Node | undefined = declaration
    while (current && !ts.isImportDeclaration(current)) current = current.parent
    if (!current || !ts.isImportDeclaration(current) || !ts.isStringLiteral(current.moduleSpecifier)) return false
    return current.moduleSpecifier.text.endsWith(`/i18n/${modules[helper]}`)
  }
  return (call) => {
    const target = unwrap(call.expression)
    if (ts.isIdentifier(target)) {
      return (checker.getSymbolAtLocation(target)?.declarations ?? []).some((declaration) => {
        if (!ts.isImportSpecifier(declaration) || declaration.isTypeOnly) return false
        const helper = (declaration.propertyName ?? declaration.name).text
        return helper in modules && matchesImport(declaration, helper)
      })
    }
    if (
      (ts.isPropertyAccessExpression(target) || ts.isElementAccessExpression(target)) &&
      ts.isIdentifier(target.expression)
    ) {
      const helper = ts.isPropertyAccessExpression(target)
        ? target.name.text
        : ts.isStringLiteralLike(target.argumentExpression)
          ? target.argumentExpression.text
          : null
      return (
        helper !== null &&
        helper in modules &&
        (checker.getSymbolAtLocation(target.expression)?.declarations ?? []).some(
          (declaration) => ts.isNamespaceImport(declaration) && matchesImport(declaration, helper),
        )
      )
    }
    return false
  }
}

/** Every unmarked raw error read in one source file. */
function findRawErrorText(fileName: string, content: string): RawErrorFinding[] {
  const source = ts.createSourceFile(
    fileName,
    content,
    ts.ScriptTarget.Latest,
    true,
    fileName.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS,
  )
  const isDirectHelper = directHelperChecker(source)
  const lines = content.split('\n')
  const findings: RawErrorFinding[] = []

  const visit = (node: ts.Node) => {
    let raw = false
    if (ts.isPropertyAccessExpression(node) && node.name.text === 'message') {
      raw = isErrorShaped(node.expression, source)
    } else if (ts.isCallExpression(node) && ts.isIdentifier(node.expression)) {
      const callee = node.expression.text
      raw =
        RAW_HELPERS.has(callee) ||
        (callee === 'String' && node.arguments.length === 1 && isErrorShaped(node.arguments[0], source))
    }
    if (raw && !hasMarker(node, source, lines)) {
      const line = source.getLineAndCharacterOfPosition(node.getStart(source)).line
      findings.push({ kind: 'raw', line: line + 1, text: lines[line].trim() })
    }
    if (ts.isCallExpression(node) && isDirectHelper(node) && !hasMarker(node, source, lines)) {
      const line = source.getLineAndCharacterOfPosition(node.getStart(source)).line
      findings.push({ kind: 'direct', line: line + 1, text: lines[line].trim() })
    }
    ts.forEachChild(node, visit)
  }

  visit(source)
  return findings
}

function collectScreenFiles(dir: string, files: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry)
    if (statSync(path).isDirectory()) {
      if (entry !== '__tests__') collectScreenFiles(path, files)
      continue
    }
    if (/\.tsx?$/.test(entry) && !/\.(test|stories)\.tsx?$/.test(entry) && !entry.endsWith('.d.ts')) {
      files.push(path)
    }
  }
  return files
}

describe('raw error text scanner (#13756)', () => {
  it.each([
    ['toast', "addToast('error', error.message)"],
    ['optional chain', 'render(this.state.error?.message)'],
    ['mutation error', 'render(scanMutation.error.message)'],
    ['cast', 'render((error as Error).message)'],
    ['suffix name', 'render(nextError.message)'],
    ['narrowed value', 'const f = (value: unknown) => (value instanceof Error && value.message ? value.message : "")'],
    ['String()', 'setError(String(e))'],
    // biome-ignore lint/suspicious/noTemplateCurlyInString: the probe is source text for the scanner, not a template
    ['template', 'addToast("error", `Failed: ${err.message}`)'],
    ['raw helper', 'setError(errorMessageFromInvoke(failure))'],
    ['typed status', "render(audioStatus.model_status.state === 'error' && audioStatus.model_status.message)"],
    ['typed branch', "if (status.state === 'error') render(status.message)"],
  ])('flags a raw read: %s', (_label, snippet) => {
    expect(findRawErrorText('probe.tsx', snippet)).not.toEqual([])
  })

  it.each([
    ['toast record', 'render(toast.message)'],
    ['result payload', 'render(result.message)'],
    ['display text', 'render(display.text)'],
    ['non-error status', "render(status.state === 'ready' && status.message)"],
    ['unrelated status', "if (other.state === 'error') render(status.message)"],
    ['translation', "render(t('errors.generic'))"],
  ])('does not flag non-error data: %s', (_label, snippet) => {
    expect(findRawErrorText('probe.tsx', snippet)).toEqual([])
  })

  it.each([
    ['same line', "if (e.message.includes('fetch')) return // raw-error-text: classification only"],
    ['line above', "// raw-error-text: classification only\nif (e.message.includes('fetch')) return"],
    [
      'statement',
      "// raw-error-text: matched, never rendered\nconst offline =\n  e instanceof TypeError &&\n  e.message.toLowerCase().includes('fetch')",
    ],
  ])('honours a reasoned marker: %s', (_label, snippet) => {
    expect(findRawErrorText('probe.ts', snippet)).toEqual([])
  })
})

describe('direct display helper import bindings (#13756)', () => {
  it.each([
    "import { translateError } from '../i18n/translateError'; render(translateError(failure, 'en'))",
    "import { translateError as localize } from '../i18n/translateError'; render(localize(failure, 'en'))",
    "import { describeIpcError as describe } from '../../i18n/tauriIpcErrors'; render(describe(failure, t))",
    "import * as errors from '../../i18n/tauriIpcErrors'; render(errors.describeIpcError(failure, t))",
    "import * as errors from '../i18n/translateError'; render(errors.translateError(failure, 'en'))",
    "import * as errors from '../i18n/translateError'; render(errors['translateError'](failure, 'en'))",
    "import * as errors from '../i18n/tauriIpcErrors'; render(errors['describeIpcError'](failure, t))",
  ])('detects a direct call through its actual import: %s', (snippet) => {
    expect(findRawErrorText('probe.tsx', snippet).map((finding) => finding.kind)).toEqual(['direct'])
  })
  it.each([
    'function translateError(value) { return value }; render(translateError(result))',
    "import { translateError } from '../other'; render(translateError(result))",
    "import { translateError } from '../i18n/translateError'; function render(translateError) { return translateError(result) }",
    "import * as errors from '../i18n/tauriIpcErrors'; function render(errors) { return errors.describeIpcError(result) }",
    "import * as errors from '../other'; render(errors['translateError'](result))",
    "import * as errors from '../i18n/translateError'; function render(errors) { return errors['translateError'](result) }",
    "import * as errors from '../i18n/translateError'; { const errors = { translateError: (value) => value }; errors['translateError'](result) }",
    "import { translateError } from '../i18n/translateError'; { const translateError = (value) => value; translateError(result) }",
  ])('ignores unrelated and shadowed bindings: %s', (snippet) => {
    expect(findRawErrorText('probe.tsx', snippet)).toEqual([])
  })
  it('counts raw reads and direct calls independently in the same file', () => {
    const snippet =
      "import { translateError } from '../i18n/translateError'; render(error.message); render(translateError(error, 'en'))"
    expect(findRawErrorText('probe.tsx', snippet).map((finding) => finding.kind)).toEqual(['raw', 'direct'])
  })
})

describe('discriminated status branch direction (#13756)', () => {
  it.each([
    "if (status.state === 'error') render(status.message); else handle()",
    "status.state === 'error' ? status.message : 'ready'",
    "if (status.state !== 'error') handle(); else render(status.message)",
    "status.state !== 'error' ? 'ready' : status.message",
    "status.state === 'error' && render(status.message)",
    "status.state !== 'error' || render(status.message)",
  ])('flags diagnostics only on a proven error branch: %s', (snippet) => {
    expect(findRawErrorText('probe.tsx', snippet).map((finding) => finding.kind)).toEqual(['raw'])
  })
  it.each([
    "if (status.state === 'error') handle(); else render(status.message)",
    "status.state === 'error' ? 'failed' : status.message",
    "if (status.state !== 'error') render(status.message); else handle()",
    "status.state === 'error' || render(status.message)",
    "if (status.state === 'error' || status.state === 'ready') render(status.message)",
    "if (!(status.state === 'error')) render(status.message)",
  ])('keeps ordinary messages outside the error branch: %s', (snippet) => {
    expect(findRawErrorText('probe.tsx', snippet)).toEqual([])
  })
})

describe('screens render errors through the shared display path (#13756)', () => {
  const findings = SCANNED_ROOTS.flatMap((root) =>
    collectScreenFiles(join(SRC_ROOT, root)).map((path) => {
      const file = relative(SRC_ROOT, path).replace(/\\/g, '/')
      return { file, findings: findRawErrorText(file, readFileSync(path, 'utf8')) }
    }),
  )

  it.each([
    'raw',
    'direct',
  ] as const)('has no new unmarked %s display bypass in pages, components or overlay', (kind) => {
    const debt = kind === 'raw' ? PENDING_RAW_DEBT : PENDING_DIRECT_DEBT
    const offenders = findings.flatMap(({ file, findings: found }) => {
      const matching = found.filter((finding) => finding.kind === kind)
      if (matching.length <= (debt[file] ?? 0)) return []
      return matching.map((finding) => `src/${file}:${finding.line} ${finding.kind} ${finding.text}`)
    })
    expect(offenders).toEqual([])
  })

  it('keeps every remaining debt entry pointed at an existing consumer', () => {
    const scanned = new Set(findings.map(({ file }) => file))
    expect(
      [...Object.keys(PENDING_RAW_DEBT), ...Object.keys(PENDING_DIRECT_DEBT)].filter((file) => !scanned.has(file)),
    ).toEqual([])
  })
})
