const { slugify } = require('./slug.js');

const a = slugify('Hello World!');
if (a !== 'hello-world') {
  throw new Error(`slugify('Hello World!') expected 'hello-world' but got '${a}'`);
}

const b = slugify('  A B  ');
if (b !== 'a-b') {
  throw new Error(`slugify('  A B  ') expected 'a-b' but got '${b}'`);
}

console.log('PASS');
