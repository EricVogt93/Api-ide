function run(ctx, input) {
  return {
    headers: [{ name: "Authorization", value: "Bearer " + input.token }]
  };
}
