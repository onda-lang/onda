const EXAMPLE_CATALOG_VERSION = 1;

export async function loadExampleProject(catalogUrl, exampleId, fetchImpl = fetch) {
  if (!validExampleId(exampleId)) {
    throw new Error("the requested playground example path is invalid");
  }
  if (!catalogUrl) {
    throw new Error("this playground host does not provide repository examples");
  }
  const response = await fetchImpl(catalogUrl);
  if (!response.ok) {
    throw new Error(`failed to load the playground examples: ${response.status}`);
  }
  const catalog = await response.json();
  if (
    !catalog
    || catalog.version !== EXAMPLE_CATALOG_VERSION
    || !catalog.projects
    || typeof catalog.projects !== "object"
    || Array.isArray(catalog.projects)
  ) {
    throw new Error("the playground example catalog is invalid");
  }
  const project = catalog.projects[exampleId];
  if (!project) throw new Error(`playground example '${exampleId}' was not found`);
  return project;
}

function validExampleId(value) {
  return typeof value === "string"
    && value.length > 0
    && value.length <= 160
    && !value.startsWith("/")
    && !value.includes("\\")
    && !value.split("/").some((segment) => !segment || segment === "." || segment === "..")
    && /\.(?:onda|on)$/.test(value);
}
