function run(ctx, input) {
  var actual = (ctx.response.body || {}).name;
  return {
    passed: actual === input.expected,
    message: "pet name matches the reusable assertion",
    expected: input.expected,
    actual: actual,
    path: "$.name"
  };
}
