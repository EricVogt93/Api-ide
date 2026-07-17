function run(ctx, input) {
  // onError asset: record that the error path ran, and echo the error.
  return { runtime: { errored: true, errorMsg: ctx.error || "" } };
}
