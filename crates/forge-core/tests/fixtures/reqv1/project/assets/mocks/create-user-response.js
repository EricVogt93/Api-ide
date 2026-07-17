function run(ctx, input) {
  return {
    status: 201,
    headers: [{ name: "Content-Type", value: "application/json" }],
    body: { id: "u-mock", name: input.user.name }
  };
}
