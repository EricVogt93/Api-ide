function run(ctx, input) {
  var runtime = {};
  runtime[input.target] = (ctx.response.body || {}).id;
  return { runtime: runtime };
}
