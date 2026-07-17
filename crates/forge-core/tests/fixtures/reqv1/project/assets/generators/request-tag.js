function run(ctx, input) {
  return input.prefix + "-" + ctx.bindings.user.name.toLowerCase();
}
