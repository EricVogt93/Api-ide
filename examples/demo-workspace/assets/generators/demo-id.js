function run(ctx, input) {
  return input.prefix + "-" + ctx.bindings.pet.name.toLowerCase();
}
