export function isExpectedTeardownNetworkProblem(problem) {
  return problem.status === "loadingFailed" &&
    problem.errorText === "net::ERR_ABORTED" &&
    problem.canceled === true;
}

export function collectUnexpectedNetworkProblems(problems) {
  return problems.filter((problem) => !isExpectedTeardownNetworkProblem(problem));
}
