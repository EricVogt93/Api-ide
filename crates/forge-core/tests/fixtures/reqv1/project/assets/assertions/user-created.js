function run(ctx, input) {
  var body = ctx.response.body || {};
  return [
    {
      passed: body.name === input.expectedUser.name,
      message: "created user has the expected name",
      expected: input.expectedUser.name,
      actual: body.name,
      path: "$.name"
    },
    {
      passed: ctx.response.status === 201,
      message: "status is 201 (checked from JS)"
    }
  ];
}
