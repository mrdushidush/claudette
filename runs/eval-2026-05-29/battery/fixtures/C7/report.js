const data = { members: [{ name: 'Ada' }, { name: 'Linus' }, { name: 'Grace' }] };

// Print each member name.
const r=data.users.map((u) => u.name);
for (const name of r) {
  console.log(name);
}
