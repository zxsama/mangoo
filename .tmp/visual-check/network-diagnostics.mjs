export function isExpectedTeardownNetworkProblem(problem) {
  return problem.phase === "teardown" &&
    problem.status === "loadingFailed" &&
    problem.errorText === "net::ERR_ABORTED" &&
    problem.canceled === true;
}

export function collectUnexpectedNetworkProblems(problems) {
  return problems.filter((problem) => !(
    problem.status === "loadingFailed" &&
    problem.errorText === "net::ERR_ABORTED" &&
    problem.canceled === true
  ));
}

export function collectUnexpectedPhaseAwareNetworkProblems(problems) {
  return problems.filter((problem) => !isExpectedTeardownNetworkProblem(problem));
}

export function isKnownElectronHarnessConsoleError(text) {
  return /sandboxed_renderer\.bundle\.js script failed to run|preloadScripts.*startupData/i.test(text);
}

export function collectUnexpectedConsoleErrors(errors) {
  return errors.filter((entry) =>
    !isKnownElectronHarnessConsoleError(typeof entry === "string" ? entry : entry?.text ?? "")
  );
}
