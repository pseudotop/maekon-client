import { describe, expect, it } from 'vitest'
import GroupedJsonReporter from '../../../e2e-tauri/grouped-json-reporter'

type Reporter = InstanceType<typeof GroupedJsonReporter>
type Emission = (reporter: Reporter, common: { cid: string; specs: string[] }) => void

function collect(specs: string[], emit: Emission, failures = 0, error?: string) {
  const chunks: string[] = []
  const reporter = new GroupedJsonReporter({
    stdout: true,
    writeStream: {
      write: (value: unknown) => {
        chunks.push(String(value))
        return true
      },
    },
  })
  const common = { cid: '0-0', specs }
  reporter.emit('runner:start', {
    ...common,
    config: { framework: 'mocha', mochaOpts: {} },
    capabilities: { browserName: 'tauri' },
    sessionId: 'synthetic',
    instanceOptions: {},
  })
  emit(reporter, common)
  reporter.emit('runner:end', { failures, retries: 0, error })
  expect(chunks).toHaveLength(1)
  return JSON.parse(chunks[0])
}

function suite(common: { cid: string; specs: string[] }, index: number) {
  return {
    ...common,
    uid: `suite-${index}`,
    title: 'same suite title',
    fullTitle: 'same suite title',
    file: common.specs[index],
  }
}

function testEvent(common: { cid: string; specs: string[] }, uid: string, parent: string) {
  return {
    ...common,
    uid,
    title: 'same test title',
    fullTitle: 'same suite title same test title',
    parent,
    pending: false,
  }
}

describe('grouped native JSON reporting (#12039)', () => {
  it.each([1, 2, 6])('records each of %i grouped spec executions exactly once', (count) => {
    const specs = Array.from({ length: count }, (_, index) => `${index}.spec.ts`)
    const report = collect(specs, (reporter, common) => {
      for (let index = 0; index < count; index++) {
        const currentSuite = suite(common, index)
        const test = testEvent(common, `test-${index}`, currentSuite.uid)
        reporter.emit('suite:start', currentSuite)
        reporter.emit('test:start', test)
        reporter.emit('test:pass', test)
        reporter.emit('test:end', test)
        reporter.emit('suite:end', currentSuite)
      }
    })
    expect(report.specs).toEqual(specs)
    expect(report.suites).toHaveLength(count)
    expect(report.state.passed).toBe(count)
    expect(report.state.failed).toBe(0)
    expect(report.suites.map((value: { name: string }) => value.name)).toEqual(Array(count).fill('same suite title'))
  })

  it('retains failed, skipped and incomplete executions without counting them as success', () => {
    const report = collect(
      ['one.spec.ts', 'two.spec.ts'],
      (reporter, common) => {
        const currentSuite = suite(common, 0)
        reporter.emit('suite:start', currentSuite)
        const failed = testEvent(common, 'failed', currentSuite.uid)
        reporter.emit('test:start', failed)
        reporter.emit('test:fail', { ...failed, error: new Error('expected failure evidence') })
        reporter.emit('test:end', failed)
        reporter.emit('test:pending', { ...testEvent(common, 'skip', currentSuite.uid), pending: true })
        reporter.emit('test:start', testEvent(common, 'unfinished', currentSuite.uid))
        reporter.emit('suite:end', currentSuite)
      },
      1,
    )
    expect(report.suites).toHaveLength(1)
    expect(report.state).toEqual({ passed: 0, failed: 1, skipped: 1, pending: 1 })
    expect(report.suites[0].tests[0].error.message).toBe('expected failure evidence')
    expect(report.suites[0].tests[2].state).toBe('pending')
  })

  it('keeps both retry attempts even when their execution UID and names match', () => {
    const report = collect(['one.spec.ts', 'two.spec.ts'], (reporter, common) => {
      const currentSuite = suite(common, 0)
      const test = testEvent(common, 'retried', currentSuite.uid)
      reporter.emit('suite:start', currentSuite)
      reporter.emit('test:start', test)
      reporter.emit('test:retry', { ...test, error: new Error('first attempt failed') })
      reporter.emit('test:start', test)
      reporter.emit('test:pass', test)
      reporter.emit('test:end', test)
      reporter.emit('suite:end', currentSuite)
    })
    const attempts = report.suites[0].tests
    expect(attempts).toHaveLength(2)
    expect(attempts.map((value: { state: string }) => value.state)).toEqual(['failed', 'passed'])
    expect(attempts.map((value: { uid: string }) => value.uid)).toEqual(['retried', 'retried'])
    expect(attempts.map((value: { retries: number }) => value.retries)).toEqual([0, 1])
    expect(attempts[0].error.message).toBe('first attempt failed')
    expect(report.state.passed).toBe(1)
    expect(report.state.failed).toBe(1)
  })

  it('preserves a root hook failure even when no suite started', () => {
    const report = collect(
      ['one.spec.ts', 'two.spec.ts'],
      (reporter, common) => {
        const hook = { ...common, uid: 'root-before', title: 'before all', parent: '(root)' }
        reporter.emit('hook:start', hook)
        reporter.emit('hook:end', { ...hook, error: new Error('bootstrap failed') })
      },
      1,
    )
    expect(report.suites).toEqual([])
    expect(report.hooks).toHaveLength(1)
    expect(report.hooks[0].error.message).toBe('bootstrap failed')
    expect(report.state.failed).toBe(1)
    expect(report.runner.failures).toBe(1)
    expect(report.status).toBe('failed')
  })

  it('retains runner-level errors without inventing successful tests', () => {
    const report = collect(['one.spec.ts'], () => {}, 1, 'worker initialization failed')
    expect(report.state.passed).toBe(0)
    expect(report.runner.error).toBe('worker initialization failed')
    expect(report.status).toBe('failed')
  })

  it('retains distinct suite executions even when the collector map reuses a UID', () => {
    const report = collect(['one.spec.ts', 'two.spec.ts'], (reporter, common) => {
      for (const uid of ['first', 'second']) {
        const currentSuite = suite(common, 0)
        const test = testEvent(common, uid, currentSuite.uid)
        reporter.emit('suite:start', currentSuite)
        reporter.emit('test:start', test)
        reporter.emit('test:pass', test)
        reporter.emit('test:end', test)
        reporter.emit('suite:end', currentSuite)
      }
    })
    expect(report.suites.map((value: { tests: { uid: string }[] }) => value.tests[0].uid)).toEqual(['first', 'second'])
    expect(report.state.passed).toBe(2)
  })

  it('preserves tests declared outside a describe block', () => {
    const report = collect(['one.spec.ts'], (reporter, common) => {
      const test = testEvent(common, 'root-test', '(root)')
      reporter.emit('test:start', test)
      reporter.emit('test:pass', test)
      reporter.emit('test:end', test)
    })
    expect(report.suites).toEqual([])
    expect(report.tests).toHaveLength(1)
    expect(report.state.passed).toBe(1)
  })

  it('does not describe an empty runner as a successful execution', () => {
    const report = collect([], () => {})
    expect(report.status).toBe('empty')
    expect(report.state.passed).toBe(0)
  })

  it.each([true, false])('distinguishes completed=%s successful hooks from unfinished hooks', (completed) => {
    const report = collect(['one.spec.ts'], (reporter, common) => {
      const hook = { ...common, uid: 'root-before', title: 'before all', parent: '(root)' }
      reporter.emit('hook:start', hook)
      if (completed) reporter.emit('hook:end', hook)
    })
    expect(report.hooks[0].state).toBe(completed ? 'passed' : 'pending')
    expect(report.state.pending).toBe(completed ? 0 : 1)
  })
})
