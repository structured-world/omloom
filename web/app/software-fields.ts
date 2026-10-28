import { FieldType } from '../src/generated/omloom/v1/workspace';
import { InstallationKind, WorkloadKind, AssetLifecycle } from '../src/generated/omloom/v1/contracts';

function choices(values: Record<number, string>, prefix: string): string[] {
  return Object.entries(values).filter(([n]) => Number(n) > 0).map(([, name]) => `${prefix}${name}`);
}
const definitions: Record<string, { name: string; choices?: string[]; date?: boolean }> = {
  publisher: { name: 'Publisher' }, raw_name: { name: 'Original product label' },
  normalization_basis: { name: 'Reviewed normalization source' },
  version: { name: 'Version' }, raw_version: { name: 'Original version label' },
  edition: { name: 'Edition' }, architecture: { name: 'Architecture' }, artifact_digest: { name: 'Artifact SHA-256 digest' },
  installation_kind: { name: 'Installation purpose', choices: choices(InstallationKind, 'INSTALLATION_KIND_') },
  installation_scope: { name: 'Installation path or package scope' },
  workload_kind: { name: 'Deployment kind', choices: choices(WorkloadKind, 'WORKLOAD_KIND_') },
  compose_project: { name: 'Compose project (grouping only)' }, compose_service: { name: 'Compose service (grouping only)' },
  compose_files: { name: 'Ordered Compose file references (one per line)' }, working_directory: { name: 'Host working directory reference' },
  repository_reference: { name: 'Repository reference (without credentials)' }, source_revision: { name: 'Source revision' },
  dockerfile_path: { name: 'Dockerfile path reference' }, invocation_reference: { name: 'Reviewed launch specification reference' },
  image_digest: { name: 'Image SHA-256 digest' }, runtime_object_id: { name: 'Runtime-local container ID' },
  runtime_lifetime: { name: 'Runtime lifetime evidence' },
  lifecycle: { name: 'Inventory lifecycle', choices: choices(AssetLifecycle, 'ASSET_LIFECYCLE_') },
  warranty_until: { name: 'Warranty through', date: true }, support_until: { name: 'Support through', date: true },
  contract_reference: { name: 'Contract reference (without keys)' },
  purpose: { name: 'Purpose' }, supplier: { name: 'Supplier' }, model: { name: 'Model reference' },
  purchase_reference: { name: 'Purchase reference' }, lifecycle_date: { name: 'Lifecycle date', date: true },
  disposal_reference: { name: 'Attributed disposal evidence reference' },
};
export function softwareFields(allowed: string[] = Object.keys(definitions)) {
  return allowed.flatMap(id => {
    const field = definitions[id];
    return field ? [{ id, name: field.name, type: field.choices ? FieldType.CHOICE : field.date ? FieldType.DATE : FieldType.TEXT, choices: field.choices ?? [], advanced: false }] : [];
  });
}
export function softwareChoiceLabel(id: string, value: string): string {
  if (!definitions[id]?.choices?.includes(value)) return value;
  return value.replace(/^(INSTALLATION_KIND_|WORKLOAD_KIND_|ASSET_LIFECYCLE_)/, '').toLowerCase().replaceAll('_', ' ');
}
const noChoices: readonly string[] = [];
export function softwareChoices(id: string): readonly string[] { return definitions[id]?.choices ?? noChoices; }
