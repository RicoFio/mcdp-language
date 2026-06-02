const fs = require("fs");
const path = require("path");
const childProcess = require("child_process");

const { checkVersions, expectedServerVersion } = require("./sync-version");

const extensionRoot = path.resolve(__dirname, "..");
const workspaceRoot = path.resolve(extensionRoot, "..", "..");
const packagePath = path.join(extensionRoot, "package.json");
const pkg = JSON.parse(fs.readFileSync(packagePath, "utf8"));
const errors = [];

function requireEqual(label, actual, expected) {
    if (actual !== expected) {
        errors.push(`${label} must be ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
    }
}

function requireFile(label, relativePath) {
    const absolutePath = path.join(extensionRoot, relativePath);
    if (!fs.existsSync(absolutePath)) {
        errors.push(`${label} is missing: ${relativePath}`);
    }
}

requireEqual("package name", pkg.name, "mcdpl-vscode");
requireEqual("publisher", pkg.publisher, "ZardiniLab");
requireEqual("display name", pkg.displayName, "MCDPL Co-Design Tools");
requireEqual("language id", pkg.contributes?.languages?.[0]?.id, "mcdpl");
for (const mismatch of checkVersions()) {
    errors.push(mismatch);
}

requireFile("extension entrypoint", pkg.main);
requireFile("extension icon", pkg.icon);
requireFile("language configuration", pkg.contributes.languages[0].configuration);
requireFile("light language icon", pkg.contributes.languages[0].icon.light);
requireFile("dark language icon", pkg.contributes.languages[0].icon.dark);
requireFile("Marketplace README", "MARKETPLACE.md");
requireFile("VSIX ignore file", ".vscodeignore");

const localLicense = path.join(extensionRoot, "LICENSE");
const workspaceLicense = path.join(workspaceRoot, "LICENSE");
if (!fs.existsSync(localLicense) && !fs.existsSync(workspaceLicense)) {
    errors.push("LICENSE is missing from the extension package and workspace root");
}
if (process.env.MCDPL_REQUIRE_PACKAGED_LICENSE === "1" && !fs.existsSync(localLicense)) {
    errors.push("LICENSE must be copied into editors/vscode-mcdpl before packaging");
}

const serverTarget = process.env.MCDPL_SERVER_TARGET;
if (serverTarget) {
    const binaryName = serverTarget.startsWith("win32") ? "mcdp-lsp.exe" : "mcdp-lsp";
    const relativeServerPath = path.join("server", serverTarget, binaryName);
    const serverPath = path.join(extensionRoot, relativeServerPath);
    requireFile(
        `${serverTarget} bundled language server`,
        relativeServerPath,
    );
    if (fs.existsSync(serverPath)) {
        const actualVersion = childProcess.execFileSync(serverPath, ["--version"], {
            encoding: "utf8",
        }).trim();
        const expectedVersion = expectedServerVersion();
        if (actualVersion !== expectedVersion) {
            errors.push(
                `${serverTarget} bundled language server version must be ${JSON.stringify(expectedVersion)}, got ${JSON.stringify(actualVersion)}`,
            );
        }
    }
}

if (errors.length > 0) {
    console.error("VSCode extension preflight failed:");
    for (const error of errors) {
        console.error(`- ${error}`);
    }
    process.exit(1);
}

console.log("VSCode extension preflight passed");
