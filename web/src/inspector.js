// Structured model editing core: catalog/schema metadata, round-trip JSON
// edits (with JSON text as the single source of truth), undo/redo history, and
// mapping validate diagnostics back to sections, items, and fields.

const EDITABLE_SECTIONS = new Set(["components", "controls", "outputs"]);

const UNIT_BY_SUFFIX = [
  ["_ohm_per_km", "ohm/km"],
  ["_h_per_km", "H/km"],
  ["_f_per_km", "F/km"],
  ["_ohm", "ohm"],
  ["_mvar", "Mvar"],
  ["_mva", "MVA"],
  ["_mw", "MW"],
  ["_kw", "kW"],
  ["_kv", "kV"],
  ["_hz", "Hz"],
  ["_deg", "deg"],
  ["_wb", "Wb"],
  ["_km", "km"],
  ["_pu", "pu"],
  ["_v", "V"],
  ["_a", "A"],
  ["_h", "H"],
  ["_f", "F"],
  ["_s", "s"],
];

function isPlainObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function cloneJson(value) {
  if (value === undefined) return undefined;
  return structuredClone(value);
}

function stripBom(text) {
  return text.charCodeAt(0) === 0xfeff ? text.slice(1) : text;
}

function formatModel(model) {
  return `${JSON.stringify(model, null, 2)}\n`;
}

function parseJsonInput(value, label) {
  if (typeof value === "string") {
    return JSON.parse(stripBom(value));
  }
  if (value && typeof value === "object") {
    return value;
  }
  throw new TypeError(`${label} must be a JSON string or object`);
}

function inferUnitFromName(name) {
  for (const [suffix, unit] of UNIT_BY_SUFFIX) {
    if (name.endsWith(suffix)) return unit;
  }
  return "";
}

function resolveRef(schemaNode, defs) {
  let current = schemaNode;
  const seen = new Set();
  while (current && typeof current.$ref === "string") {
    const ref = current.$ref;
    if (seen.has(ref)) break;
    seen.add(ref);
    const name = ref.replace(/^#\/(?:\$defs|definitions)\//, "");
    const target = defs[name];
    if (!target) break;
    // Preserve outer description/default when wrapping a $ref.
    current = {
      ...target,
      ...(current.description !== undefined ? { description: current.description } : {}),
      ...(current.default !== undefined ? { default: current.default } : {}),
    };
  }
  return current;
}

function unwrapNullable(schemaNode, defs) {
  const resolved = resolveRef(schemaNode, defs);
  if (!resolved) return { schema: {}, nullable: false, refName: null };

  let refName = null;
  if (typeof schemaNode?.$ref === "string") {
    refName = schemaNode.$ref.replace(/^#\/(?:\$defs|definitions)\//, "");
  }

  if (Array.isArray(resolved.type)) {
    const nonNull = resolved.type.filter((t) => t !== "null");
    if (nonNull.length === 1 && resolved.type.includes("null")) {
      return {
        schema: { ...resolved, type: nonNull[0] },
        nullable: true,
        refName,
      };
    }
  }

  if (Array.isArray(resolved.anyOf)) {
    const nonNullBranches = resolved.anyOf.filter((b) => b?.type !== "null");
    if (nonNullBranches.length === 1 && nonNullBranches.length < resolved.anyOf.length) {
      const inner = nonNullBranches[0];
      const innerResolved = resolveRef(inner, defs);
      const innerRefName = typeof inner?.$ref === "string"
        ? inner.$ref.replace(/^#\/(?:\$defs|definitions)\//, "")
        : refName;
      return {
        schema: {
          ...innerResolved,
          ...(resolved.description !== undefined ? { description: resolved.description } : {}),
          ...(resolved.default !== undefined ? { default: resolved.default } : {}),
        },
        nullable: true,
        refName: innerRefName,
      };
    }
    if (nonNullBranches.length > 1 && nonNullBranches.length < resolved.anyOf.length) {
      return {
        schema: {
          ...resolved,
          anyOf: nonNullBranches,
        },
        nullable: true,
        refName,
      };
    }
  }

  return { schema: resolved, nullable: false, refName };
}

function extractEnumValues(schemaNode) {
  if (Array.isArray(schemaNode.enum)) {
    return [...schemaNode.enum];
  }
  if (Array.isArray(schemaNode.oneOf) && schemaNode.oneOf.length > 0) {
    const allConst = schemaNode.oneOf.every(
      (v) => v && typeof v.const === "string" && (v.type === undefined || v.type === "string"),
    );
    if (allConst) {
      return schemaNode.oneOf.map((v) => v.const);
    }
  }
  return null;
}

function buildObjectFields(objectSchema, defs) {
  const props = objectSchema.properties ?? {};
  const required = new Set(objectSchema.required ?? []);
  const fields = [];
  for (const [name, prop] of Object.entries(props)) {
    if (name === "type") continue;
    fields.push(buildFieldMeta(name, {
      name,
      unit: inferUnitFromName(name),
      required: required.has(name),
      description: prop.description ?? "",
    }, prop, required, defs));
  }
  return fields;
}

function buildFieldMeta(name, param, propSchema, requiredSet, defs) {
  const { schema: resolved, nullable, refName } = unwrapNullable(propSchema ?? {}, defs);
  const unit = param?.unit ?? inferUnitFromName(name);
  const required = param?.required ?? requiredSet.has(name);
  const description = param?.description || resolved.description || "";

  const field = {
    name,
    unit,
    required,
    description,
    kind: "string",
  };

  if (nullable) field.nullable = true;
  if (resolved.default !== undefined) field.default = cloneJson(resolved.default);
  if (typeof resolved.minimum === "number") field.minimum = resolved.minimum;
  if (typeof resolved.exclusiveMinimum === "number") field.exclusiveMinimum = resolved.exclusiveMinimum;
  if (typeof resolved.maximum === "number") field.maximum = resolved.maximum;
  if (typeof resolved.exclusiveMaximum === "number") field.exclusiveMaximum = resolved.exclusiveMaximum;

  const enumValues = extractEnumValues(resolved);
  if (enumValues) {
    field.kind = "enum";
    field.enumValues = enumValues;
    return field;
  }

  if (refName === "BusRef") {
    field.kind = "bus";
    field.itemSchema = { kind: "string" };
    return field;
  }

  if (Array.isArray(resolved.anyOf)) {
    const types = resolved.anyOf.map((branch) => resolveRef(branch, defs)?.type).filter(Boolean);
    if (types.includes("number") && types.includes("string")) {
      field.kind = "number_or_signal";
      return field;
    }
  }

  if (Array.isArray(resolved.oneOf)) {
    const variants = resolved.oneOf
      .filter((v) => v?.properties?.type?.const !== undefined)
      .map((variant) => ({
        type: variant.properties.type.const,
        description: variant.description ?? "",
        fields: buildObjectFields(variant, defs),
      }));
    if (variants.length > 0) {
      field.kind = "object";
      field.variants = variants;
      return field;
    }
  }

  if (resolved.type === "array") {
    field.kind = name === "nodes" ? "nodes" : "array";
    if (typeof resolved.minItems === "number") field.minItems = resolved.minItems;
    if (typeof resolved.maxItems === "number") field.maxItems = resolved.maxItems;
    if (resolved.items) {
      field.itemSchema = buildFieldMeta(`${name}[]`, { unit, required: true, description: "" }, resolved.items, new Set(), defs);
    }
    return field;
  }

  if (resolved.type === "object") {
    field.kind = "object";
    field.fields = buildObjectFields(resolved, defs);
    return field;
  }

  if (resolved.type === "number" || resolved.type === "integer" || resolved.type === "boolean" || resolved.type === "string") {
    field.kind = resolved.type;
    return field;
  }

  return field;
}

function indexSchemaVariants(oneOfList) {
  const map = new Map();
  for (const variant of oneOfList ?? []) {
    const typeName = variant?.properties?.type?.const;
    if (typeof typeName === "string") {
      map.set(typeName, variant);
    }
  }
  return map;
}

function buildSimulationFields(simulationSchema, defs) {
  if (!simulationSchema?.properties) return [];
  const required = new Set(simulationSchema.required ?? []);
  const propNames = Object.keys(simulationSchema.properties);
  const ordered = [
    ...propNames.filter((n) => required.has(n)),
    ...propNames.filter((n) => !required.has(n)),
  ];
  return ordered.map((name) => {
    const prop = simulationSchema.properties[name];
    return buildFieldMeta(name, {
      name,
      unit: inferUnitFromName(name),
      required: required.has(name),
      description: prop.description ?? "",
    }, prop, required, defs);
  });
}

function buildOutputTypes(outputSchema, defs) {
  const variants = outputSchema?.oneOf ?? [];
  return variants
    .filter((v) => v?.properties?.type?.const !== undefined)
    .map((variant) => ({
      type: variant.properties.type.const,
      description: variant.description ?? "",
      fields: buildObjectFields(variant, defs),
    }));
}

/// Builds a searchable index of all component and control types from
/// `wasm.components()` and `wasm.schema()`, together with `simulation` and
/// `outputs` schema metadata.
export function buildCatalogIndex(catalog, schema) {
  const catalogList = parseJsonInput(catalog, "catalog");
  const schemaDoc = parseJsonInput(schema, "schema");
  if (!Array.isArray(catalogList)) {
    throw new TypeError("catalog must be an array");
  }
  const defs = schemaDoc?.$defs ?? schemaDoc?.definitions ?? {};
  const componentSchemas = indexSchemaVariants(defs.Component?.oneOf);
  const controlSchemas = indexSchemaVariants(defs.Control?.oneOf);

  const byType = new Map();
  const components = [];
  const controls = [];
  const all = [];

  for (const raw of catalogList) {
    const type = raw.type;
    const category = raw.category;
    const variant = category === "circuit"
      ? componentSchemas.get(type)
      : controlSchemas.get(type);
    const props = variant?.properties ?? {};
    const requiredSet = new Set(variant?.required ?? []);

    const fields = [];
    const seenFields = new Set();
    for (const param of raw.parameters ?? []) {
      seenFields.add(param.name);
      fields.push(buildFieldMeta(param.name, param, props[param.name], requiredSet, defs));
    }
    for (const [propName, propSchema] of Object.entries(props)) {
      if (propName === "type" || seenFields.has(propName)) continue;
      fields.push(buildFieldMeta(propName, null, propSchema, requiredSet, defs));
    }

    const entry = {
      type,
      category,
      summary: raw.summary ?? "",
      signConvention: raw.sign_convention ?? "",
      example: cloneJson(raw.example),
      fields,
    };
    byType.set(type, entry);
    all.push(entry);
    if (category === "circuit") components.push(entry);
    else if (category === "control") controls.push(entry);
  }

  const simulation = buildSimulationFields(defs.Simulation, defs);
  const outputs = buildOutputTypes(defs.Output, defs);

  return {
    byType,
    components,
    controls,
    all,
    simulation,
    outputs,
    get(type) {
      return byType.get(type) ?? null;
    },
    has(type) {
      return byType.has(type);
    },
  };
}

/// Returns the field definitions for the top-level `simulation` object.
export function getSimulationFields(index) {
  if (Array.isArray(index?.simulation)) return index.simulation;
  const defs = index?.$defs ?? index?.definitions;
  if (defs?.Simulation) return buildSimulationFields(defs.Simulation, defs);
  return [];
}

/// Returns the variant definitions for `outputs` items.
export function getOutputTypes(index) {
  if (Array.isArray(index?.outputs)) return index.outputs;
  const defs = index?.$defs ?? index?.definitions;
  if (defs?.Output) return buildOutputTypes(defs.Output, defs);
  return [];
}

/// Safely parses model JSON text for structured editing.
export function parseModelForEdit(text) {
  if (typeof text !== "string") {
    return { ok: false, error: "model text must be a string" };
  }
  try {
    const model = JSON.parse(stripBom(text));
    if (!isPlainObject(model)) {
      return { ok: false, error: "model must be a JSON object" };
    }
    return { ok: true, model };
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : String(err) };
  }
}

function editModel(text, mutate) {
  const parsed = parseModelForEdit(text);
  if (!parsed.ok) return parsed;
  const model = cloneJson(parsed.model);
  const problem = mutate(model);
  if (problem) return problem;
  return { ok: true, text: formatModel(model), model };
}

function validateSectionAndIndex(model, section, index) {
  if (!EDITABLE_SECTIONS.has(section)) {
    return { ok: false, error: `invalid section: ${String(section)}` };
  }
  const list = model[section];
  if (!Array.isArray(list)) {
    return { ok: false, error: `section "${section}" is not an array` };
  }
  if (!Number.isInteger(index) || index < 0 || index >= list.length) {
    return { ok: false, error: `index ${String(index)} out of range for "${section}"` };
  }
  if (!isPlainObject(list[index])) {
    return { ok: false, error: `${section}[${index}] is not an object` };
  }
  return null;
}

/// Updates a field of `simulation` (`value === undefined` removes it).
export function updateSimulationField(text, field, value) {
  if (typeof field !== "string" || field.length === 0) {
    return { ok: false, error: "field name must be a non-empty string" };
  }
  return editModel(text, (model) => {
    if (value === undefined) {
      if (isPlainObject(model.simulation)) {
        delete model.simulation[field];
      }
      return null;
    }
    if (!isPlainObject(model.simulation)) {
      model.simulation = {};
    }
    model.simulation[field] = cloneJson(value);
    return null;
  });
}

/// Merges `patch` into `model[section][index]` (`undefined` values remove keys).
export function updateItem(text, section, index, patch) {
  if (!isPlainObject(patch)) {
    return { ok: false, error: "patch must be a plain object" };
  }
  return editModel(text, (model) => {
    const err = validateSectionAndIndex(model, section, index);
    if (err) return err;
    const item = model[section][index];
    for (const [key, val] of Object.entries(patch)) {
      if (val === undefined) delete item[key];
      else item[key] = cloneJson(val);
    }
    return null;
  });
}

/// Sets a single field on `model[section][index]` (`value === undefined` removes it).
export function setItemField(text, section, index, field, value) {
  if (typeof field !== "string" || field.length === 0) {
    return { ok: false, error: "field name must be a non-empty string" };
  }
  return editModel(text, (model) => {
    const err = validateSectionAndIndex(model, section, index);
    if (err) return err;
    const item = model[section][index];
    if (value === undefined) delete item[field];
    else item[field] = cloneJson(value);
    return null;
  });
}

function collectUsedNames(model) {
  const used = new Set();
  const prefixes = new Set();
  for (const section of ["components", "controls"]) {
    if (!Array.isArray(model[section])) continue;
    for (const item of model[section]) {
      if (typeof item?.name === "string" && item.name.length > 0) {
        used.add(item.name);
        const dot = item.name.indexOf(".");
        if (dot > 0) prefixes.add(item.name.slice(0, dot));
      }
    }
  }
  return { used, prefixes };
}

function isNameConflicting(name, usedInfo) {
  if (usedInfo.used.has(name) || usedInfo.prefixes.has(name)) return true;
  const dot = name.indexOf(".");
  return dot > 0 && usedInfo.used.has(name.slice(0, dot));
}

function nextUniqueName(baseName, usedInfo) {
  if (!isNameConflicting(baseName, usedInfo)) return baseName;
  const match = /^(.*?)(\d+)$/.exec(baseName);
  const prefix = match ? match[1] : baseName;
  let counter = match ? Number(match[2]) + 1 : 2;
  while (isNameConflicting(`${prefix}${counter}`, usedInfo)) {
    counter += 1;
  }
  return `${prefix}${counter}`;
}

function lookupCatalogType(catalogIndex, typeName) {
  if (!catalogIndex) return null;
  if (typeof catalogIndex.get === "function") return catalogIndex.get(typeName);
  if (catalogIndex.byType instanceof Map) return catalogIndex.byType.get(typeName) ?? null;
  if (isPlainObject(catalogIndex)) return catalogIndex[typeName] ?? null;
  return null;
}

/// Adds an item to `"components"`, `"controls"`, or `"outputs"`.
/// When `typeOrItem` is a catalog type name, its example is cloned and given a
/// unique sequential name (e.g. `R2`, `PI2`) if that name is already in use.
export function addItem(text, section, typeOrItem, catalogIndex, options = {}) {
  if (!EDITABLE_SECTIONS.has(section)) {
    return { ok: false, error: `invalid section: ${String(section)}` };
  }
  return editModel(text, (model) => {
    let item;
    if (typeof typeOrItem === "string") {
      if (section === "outputs") {
        const outputTypes = getOutputTypes(catalogIndex);
        if (outputTypes.length > 0 && !outputTypes.some((o) => o.type === typeOrItem)) {
          return { ok: false, error: `unknown output type: ${typeOrItem}` };
        }
        item = { type: typeOrItem };
      } else {
        const def = lookupCatalogType(catalogIndex, typeOrItem);
        if (!def) {
          return { ok: false, error: `unknown type: ${typeOrItem}` };
        }
        const expectedCategory = section === "components" ? "circuit" : "control";
        if (def.category !== expectedCategory) {
          return {
            ok: false,
            error: `type "${typeOrItem}" belongs to "${def.category}", not "${section}"`,
          };
        }
        item = cloneJson(def.example);
      }
    } else if (isPlainObject(typeOrItem)) {
      item = cloneJson(typeOrItem);
    } else {
      return { ok: false, error: "typeOrItem must be a type name or an item object" };
    }

    if (isPlainObject(options?.patch)) {
      for (const [key, val] of Object.entries(options.patch)) {
        if (val === undefined) delete item[key];
        else item[key] = cloneJson(val);
      }
    }

    if (section === "components" || section === "controls") {
      const usedInfo = collectUsedNames(model);
      if (options?.name !== undefined) {
        item.name = options.ensureUnique
          ? nextUniqueName(String(options.name), usedInfo)
          : String(options.name);
      } else if (typeof item.name === "string" && item.name.length > 0 && isNameConflicting(item.name, usedInfo)) {
        item.name = nextUniqueName(item.name, usedInfo);
      }
    }

    if (!Array.isArray(model[section])) {
      model[section] = [];
    }
    const list = model[section];
    if (Number.isInteger(options?.index) && options.index >= 0 && options.index <= list.length) {
      list.splice(options.index, 0, item);
    } else {
      list.push(item);
    }
    return null;
  });
}

/// Removes the item at `index` from `"components"`, `"controls"`, or `"outputs"`.
export function removeItem(text, section, index) {
  return editModel(text, (model) => {
    const err = validateSectionAndIndex(model, section, index);
    if (err) return err;
    model[section].splice(index, 1);
    return null;
  });
}

function sameOutputSpec(a, b) {
  if (!isPlainObject(a) || !isPlainObject(b)) return false;
  const keysA = Object.keys(a).filter((k) => a[k] !== undefined).sort();
  const keysB = Object.keys(b).filter((k) => b[k] !== undefined).sort();
  if (keysA.length !== keysB.length) return false;
  return keysA.every((k, idx) => k === keysB[idx] && a[k] === b[k]);
}

/// Toggles `outputSpec` in `model.outputs` (adds if absent, removes if present).
///
/// When `outputs` is omitted (`undefined` or `null`, which in OpenKeito means
/// every node voltage, branch current, and signal is recorded implicitly),
/// toggling creates an explicit `outputs` array `[outputSpec]`. When the last
/// entry is removed, `outputs` is kept as `[]` rather than deleted, so
/// turning off the final output does not revert to recording everything.
export function toggleOutputForTarget(text, outputSpec) {
  if (!isPlainObject(outputSpec) || typeof outputSpec.type !== "string" || outputSpec.type.length === 0) {
    return { ok: false, error: "outputSpec must be an object with a non-empty type" };
  }
  return editModel(text, (model) => {
    if (!Array.isArray(model.outputs)) {
      model.outputs = [cloneJson(outputSpec)];
      return null;
    }
    const existingIndex = model.outputs.findIndex((entry) => sameOutputSpec(entry, outputSpec));
    if (existingIndex >= 0) {
      model.outputs.splice(existingIndex, 1);
    } else {
      model.outputs.push(cloneJson(outputSpec));
    }
    return null;
  });
}

/// Undo/redo history over model JSON text snapshots.
export function createEditHistory(initialText, limit = 100) {
  const maxUndo = Number.isFinite(limit) ? Math.max(0, Math.floor(limit)) : 100;
  let entries = [{ text: String(initialText ?? ""), label: null }];
  let cursor = 0;

  return {
    current() {
      return entries[cursor].text;
    },
    currentLabel() {
      return entries[cursor].label;
    },
    canUndo() {
      return cursor > 0;
    },
    canRedo() {
      return cursor < entries.length - 1;
    },
    push(text, label = null) {
      const nextText = String(text);
      if (nextText === entries[cursor].text) return false;
      if (cursor < entries.length - 1) {
        entries = entries.slice(0, cursor + 1);
      }
      entries.push({ text: nextText, label: label ?? null });
      cursor = entries.length - 1;
      while (entries.length > maxUndo + 1) {
        entries.shift();
        cursor -= 1;
      }
      return true;
    },
    undo() {
      if (cursor <= 0) return null;
      cursor -= 1;
      return entries[cursor].text;
    },
    redo() {
      if (cursor >= entries.length - 1) return null;
      cursor += 1;
      return entries[cursor].text;
    },
    reset(text) {
      const nextText = String(text ?? "");
      entries = [{ text: nextText, label: null }];
      cursor = 0;
      return nextText;
    },
  };
}

function firstFieldName(relativePath) {
  if (!relativePath) return null;
  const match = /^([^.[\]]+)/.exec(relativePath);
  return match ? match[1] : null;
}

function matchesField(relativePath, field) {
  if (!relativePath || !field) return false;
  return (
    relativePath === field ||
    relativePath.startsWith(`${field}.`) ||
    relativePath.startsWith(`${field}[`)
  );
}

function parseTargetPath(targetPath) {
  if (!targetPath || /^line \d+ column \d+$/.test(targetPath)) {
    return { section: null, index: null, field: null, relativePath: null };
  }

  const itemMatch = /^(components|controls|outputs)\[(\d+)\](?:\.(.*))?$/.exec(targetPath);
  if (itemMatch) {
    const relativePath = itemMatch[3] ?? null;
    return {
      section: itemMatch[1],
      index: Number(itemMatch[2]),
      field: firstFieldName(relativePath),
      relativePath,
    };
  }

  const sectionMatch = /^(simulation|frequency_scan)(?:\.(.*))?$/.exec(targetPath);
  if (sectionMatch) {
    const relativePath = sectionMatch[2] ?? null;
    return {
      section: sectionMatch[1],
      index: null,
      field: firstFieldName(relativePath),
      relativePath,
    };
  }

  if (EDITABLE_SECTIONS.has(targetPath)) {
    return { section: targetPath, index: null, field: null, relativePath: null };
  }

  return {
    section: null,
    index: null,
    field: firstFieldName(targetPath),
    relativePath: targetPath,
  };
}

/// Maps `validate` errors and warnings (supporting both `E001` `json_path` and
/// regular validation `path`) to section/index/field lookups.
export function mapDiagnosticsToFields(diagnostics) {
  const rawList = Array.isArray(diagnostics)
    ? diagnostics
    : [...(diagnostics?.errors ?? []), ...(diagnostics?.warnings ?? [])];

  const all = [];
  const errors = [];
  const warnings = [];
  const unmapped = [];
  const byPath = new Map();
  const byItemKey = new Map();
  const bySection = new Map();

  const pushToMap = (map, key, value) => {
    if (!map.has(key)) map.set(key, []);
    map.get(key).push(value);
  };

  for (const diag of rawList) {
    if (!diag) continue;
    const targetPath = typeof diag.json_path === "string" && diag.json_path.length > 0
      ? diag.json_path
      : (typeof diag.path === "string" ? diag.path : "");
    const parsed = parseTargetPath(targetPath);
    const entry = {
      ...diag,
      targetPath,
      section: parsed.section,
      index: parsed.index,
      field: parsed.field,
      relativePath: parsed.relativePath,
    };

    all.push(entry);
    if (entry.severity === "warning") warnings.push(entry);
    else errors.push(entry);

    if (targetPath) pushToMap(byPath, targetPath, entry);
    if (parsed.section) {
      pushToMap(bySection, parsed.section, entry);
      if (parsed.index !== null) {
        pushToMap(byItemKey, `${parsed.section}[${parsed.index}]`, entry);
      }
    } else if (!parsed.relativePath) {
      unmapped.push(entry);
    }
  }

  return {
    all,
    errors,
    warnings,
    unmapped,
    byPath,
    forSection(section) {
      return bySection.get(section) ?? [];
    },
    forItem(section, index) {
      if (index === null || index === undefined) {
        return bySection.get(section) ?? [];
      }
      return byItemKey.get(`${section}[${index}]`) ?? [];
    },
    forField(section, index, field) {
      const items = byItemKey.get(`${section}[${index}]`) ?? [];
      return items.filter((d) => matchesField(d.relativePath, field));
    },
    forSimulationField(field) {
      const sim = bySection.get("simulation") ?? [];
      return sim.filter((d) => matchesField(d.relativePath, field));
    },
  };
}
