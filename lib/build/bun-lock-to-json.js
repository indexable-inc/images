const [lockPath, registryUrl = "https://registry.npmjs.org"] = process.argv.slice(2);

if (!lockPath) {
  throw new Error("usage: bun-lock-to-json.js <bun.lock> [registry-url]");
}

const stripTrailingCommas = (text) => {
  let output = "";
  let inString = false;
  let escaped = false;

  for (let index = 0; index < text.length; index += 1) {
    const char = text[index];

    if (inString) {
      output += char;

      if (escaped) {
        escaped = false;
      } else if (char === "\\") {
        escaped = true;
      } else if (char === "\"") {
        inString = false;
      }

      continue;
    }

    if (char === "\"") {
      inString = true;
      output += char;
      continue;
    }

    if (char === ",") {
      const next = text.slice(index + 1).match(/^\s*([\]}])/);

      if (next) {
        continue;
      }
    }

    output += char;
  }

  return output;
};

const parseLock = (text) => {
  try {
    return JSON.parse(stripTrailingCommas(text));
  } catch (error) {
    throw new Error(`failed to parse bun.lock: ${error.message}`);
  }
};

const splitPackageId = (packageId) => {
  const versionSeparator = packageId.lastIndexOf("@");

  if (versionSeparator <= 0) {
    throw new Error(`unsupported Bun package id: ${packageId}`);
  }

  const name = packageId.slice(0, versionSeparator);
  const version = packageId.slice(versionSeparator + 1);

  if (!name || !version) {
    throw new Error(`unsupported Bun package id: ${packageId}`);
  }

  return { name, version };
};

const tarballNameFor = (name) => {
  if (name.startsWith("@")) {
    const parts = name.split("/");

    if (parts.length !== 2 || !parts[0] || !parts[1]) {
      throw new Error(`unsupported scoped package name: ${name}`);
    }

    return parts[1];
  }

  return name;
};

const lock = parseLock(await Bun.file(lockPath).text());

if (lock.lockfileVersion !== 1) {
  throw new Error(`unsupported Bun lockfileVersion: ${lock.lockfileVersion}`);
}

const packages = Object.entries(lock.packages ?? {}).map(([key, entry]) => {
  if (!Array.isArray(entry) || entry.length < 4) {
    throw new Error(`unsupported Bun lock entry for ${key}`);
  }

  const [packageId, source, _metadata, integrity] = entry;

  if (source !== "") {
    throw new Error(`unsupported non-registry Bun package source for ${key}: ${source}`);
  }

  if (typeof integrity !== "string" || !integrity.startsWith("sha")) {
    throw new Error(`missing integrity hash for Bun package ${key}`);
  }

  const { name, version } = splitPackageId(packageId);
  const tarballName = tarballNameFor(name);

  return {
    key,
    name,
    version,
    integrity,
    cachePath: `${name}@${version}@@@1`,
    url: `${registryUrl.replace(/\/$/, "")}/${name}/-/${tarballName}-${version}.tgz`,
  };
});

packages.sort((left, right) => left.cachePath.localeCompare(right.cachePath));

console.log(JSON.stringify({ packages }, null, 2));
