function run(ctx, input) {
  return {
    status: 201,
    headers: [{ name: "Content-Type", value: "application/json" }],
    body: { id: input.id, name: input.pet.name, tag: input.pet.tag }
  };
}
