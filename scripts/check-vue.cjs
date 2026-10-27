// Vue's compiler plugin uses the TypeScript 6 compiler API. The contract build
// retains the native TypeScript compiler selected by the root workspace.
require('vue-tsc').run(require.resolve('typescript-vue/lib/tsc'));
