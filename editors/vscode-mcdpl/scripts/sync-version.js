const childProcess = require("child_process");
const fs = require("fs");
const path = require("path");

const extensionRoot = path.resolve(__dirname, "..");
const workspaceRoot = path.resolve(extensionRoot, "..", "..");
const packagePath = path.join(extensionRoot, "package.json");
const packageLockPath = path.join(extensionRoot, "package-lock.json");

function readJson(filePath) {
    return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function writeJson(filePath, value) {
    fs.writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`);
}

function workspaceVersion() {
    const metadata = JSON.parse(
        childProcess.execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
            cwd: workspaceRoot,
            encoding: "utf8",
        }),
    );
    const package = metadata.packages.find((candidate) => candidate.name === "mcdp-lsp");
    if (!package) {
        throw new Error("cargo metadata did not include the mcdp-lsp package");
    }
    return package.version;
}

function versionState() {
    const version = workspaceVersion();
    const packageJson = readJson(packagePath);
    const packageLock = readJson(packageLockPath);
    const mismatches = [];

    if (packageJson.version !== version) {
        mismatches.push(`package.json version is ${packageJson.version}, expected ${version}`);
    }
    if (packageLock.version !== version) {
        mismatches.push(`package-lock.json version is ${packageLock.version}, expected ${version}`);
    }
    if (packageLock.packages?.[""]?.version !== version) {
        mismatches.push(
            `package-lock.json root package version is ${packageLock.packages?.[""]?.version}, expected ${version}`,
        );
    }

    return { version, packageJson, packageLock, mismatches };
}

function checkVersions() {
    return versionState().mismatches;
}

function syncVersions() {
    const { version, packageJson, packageLock } = versionState();
    const originalPackageJson = JSON.stringify(packageJson);
    const originalPackageLock = JSON.stringify(packageLock);

    packageJson.version = version;
    packageLock.version = version;
    if (!packageLock.packages) {
        packageLock.packages = {};
    }
    if (!packageLock.packages[""]) {
        packageLock.packages[""] = {};
    }
    packageLock.packages[""].version = version;

    const changed =
        JSON.stringify(packageJson) !== originalPackageJson ||
        JSON.stringify(packageLock) !== originalPackageLock;
    if (changed) {
        writeJson(packagePath, packageJson);
        writeJson(packageLockPath, packageLock);
    }

    return { version, changed };
}

function expectedServerVersion() {
    return `mcdp-lsp ${workspaceVersion()}`;
}

function main() {
    const check = process.argv.includes("--check");
    if (check) {
        const mismatches = checkVersions();
        if (mismatches.length > 0) {
            console.error("VSCode extension version is out of sync with Cargo:");
            for (const mismatch of mismatches) {
                console.error(`- ${mismatch}`);
            }
            process.exit(1);
        }
        console.log(`VSCode extension version matches Cargo ${workspaceVersion()}`);
        return;
    }

    const { version, changed } = syncVersions();
    const action = changed ? "updated" : "already matched";
    console.log(`VSCode extension version ${action} Cargo ${version}`);
}

module.exports = {
    checkVersions,
    expectedServerVersion,
    syncVersions,
    workspaceVersion,
};

if (require.main === module) {
    main();
}
