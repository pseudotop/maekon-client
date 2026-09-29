import JsonReporter from '@wdio/json-reporter'

type Suite = JsonReporter['currentSuites'][number]
type Test = Suite['tests'][number]
type Hook = Suite['hooks'][number]

function errorRecord(error?: Error) {
  return error ? { ...error, name: error.name, message: error.message, stack: error.stack } : undefined
}

function testRecord(test: Test) {
  return {
    uid: test.uid,
    name: test.title,
    start: test.start,
    end: test.end,
    duration: test.duration,
    state: test.state,
    retries: test.retries,
    pendingReason: test.pendingReason,
    error: errorRecord(test.error),
    errors: test.errors?.map(errorRecord),
  }
}

function hookRecord(hook: Hook) {
  return {
    uid: hook.uid,
    title: hook.title,
    start: hook.start,
    end: hook.end,
    duration: hook.duration,
    associatedSuite: hook.parent,
    associatedTest: hook.currentTest,
    state: hook.state ?? (hook.end ? 'passed' : 'pending'),
    error: errorRecord(hook.error),
    errors: hook.errors?.map(errorRecord),
  }
}

function descendants(suite: Suite): Suite[] {
  return suite.suites.flatMap((child) => [child, ...descendants(child)])
}

/**
 * #12039: @wdio/json-reporter 9.30 repeats every suite for each grouped spec.
 * Walk the actual execution tree once. A UID/name/timestamp is not a dedup key:
 * retries and identically named tests remain separate collected objects.
 */
export default class GroupedJsonReporter extends JsonReporter {
  override onRunnerEnd(runner: Parameters<JsonReporter['onRunnerEnd']>[0]) {
    const root = this.currentSuites[0]
    if (!root) throw new Error('Native JSON reporter lost its execution root')
    const suites = descendants(root).map((suite) => ({
      uid: suite.uid,
      name: suite.title,
      file: suite.file,
      duration: suite.duration,
      start: suite.start,
      end: suite.end,
      sessionId: runner.sessionId,
      tests: suite.tests.map(testRecord),
      hooks: suite.hooks.map(hookRecord),
    }))
    // Framework root tests/hooks are absent from the upstream suites map.
    const tests = root.tests.map(testRecord)
    const hooks = root.hooks.map(hookRecord)
    const executions = [...tests, ...suites.flatMap((suite) => suite.tests)]
    const allHooks = [...hooks, ...suites.flatMap((suite) => suite.hooks)]
    const state = {
      passed: executions.filter((test) => test.state === 'passed').length,
      failed:
        executions.filter((test) => test.state === 'failed').length +
        allHooks.filter((hook) => hook.state === 'failed').length,
      skipped: executions.filter((test) => test.state === 'skipped').length,
      pending:
        executions.filter((test) => test.state === 'pending').length +
        allHooks.filter((hook) => hook.state === 'pending').length,
    }
    const status =
      state.failed || runner.failures || runner.error
        ? 'failed'
        : state.pending
          ? 'pending'
          : executions.length
            ? 'passed'
            : 'empty'
    this.write(
      JSON.stringify({
        start: runner.start,
        end: runner.end,
        capabilities: runner.capabilities,
        framework: runner.config.framework,
        mochaOpts: runner.config.mochaOpts,
        specs: [...runner.specs],
        suites,
        tests,
        hooks,
        state,
        status,
        runner: {
          cid: runner.cid,
          sessionId: runner.sessionId,
          failures: runner.failures,
          retries: runner.retries,
          retry: runner.retry,
          error: runner.error,
        },
      }),
    )
  }
}
