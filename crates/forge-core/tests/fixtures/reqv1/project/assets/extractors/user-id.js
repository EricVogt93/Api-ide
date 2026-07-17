function run(ctx, input) {
  var out = {};
  out[input.target] = (ctx.response.body || {}).id;
  return { runtime: out };
}
