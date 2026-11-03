import { buildSync } from 'esbuild';
import { GrpcWebFetchTransport } from '@protobuf-ts/grpcweb-transport';

let module;
function contracts() {
  if (!module) {
    const built = buildSync({ stdin: { contents: 'export * from "./web/src/generated/omloom/v1/workspace"; export {AssetKind, RelationKind, EffectOutcome, OperationState, NullValue} from "./web/src/generated/omloom/v1/contracts";', resolveDir: process.cwd() }, bundle: true, write: false, format: 'esm', platform: 'node' });
    module = import('data:text/javascript;base64,' + Buffer.from(built.outputFiles[0].contents).toString('base64'));
  }
  return module;
}
export async function workspaceClient(url) {
  const m = await contracts();
  const transport = new GrpcWebFetchTransport({ baseUrl: url, format: 'binary' });
  async function revision() {
    const q = await transport.unary(m.Workspace.methods[0], m.Request.create({ command: { oneofKind: 'query', query: m.Query.create({ limit: 1 }) } }), transport.mergeOptions());
    return q.response.result.page.revision;
  }
  async function rpc(command, focus) {
    return (await transport.unary(m.Workspace.methods[0], m.Request.create({ command, working: { expected: await revision(), focus } }), transport.mergeOptions())).response.result;
  }
  return { m, rpc, revision };
}
