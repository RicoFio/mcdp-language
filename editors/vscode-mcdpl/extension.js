const fs = require("fs");
const path = require("path");
const vscode = require("vscode");
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");

const MCDPL_DOCUMENT_SELECTOR = [
    { scheme: "file", language: "mcdpl" },
    { scheme: "untitled", language: "mcdpl" },
];

let client;

function activate(context) {
    const outputChannel = vscode.window.createOutputChannel(
        "MCDPL Language Server",
        { log: true },
    );
    context.subscriptions.push(outputChannel);

    client = createLanguageClient(context, outputChannel);
    context.subscriptions.push(client.start());
}

function deactivate() {
    if (!client) {
        return undefined;
    }

    return client.stop();
}

function createLanguageClient(context, outputChannel) {
    return new LanguageClient(
        "mcdpl",
        "MCDPL Language Server",
        serverOptions(context),
        {
            documentSelector: MCDPL_DOCUMENT_SELECTOR,
            outputChannel,
            synchronize: {
                fileEvents: vscode.workspace.createFileSystemWatcher(
                    "**/*.{mcdp,mcdp_interface,mcdp_poset,mcdp_template}",
                ),
            },
        },
    );
}

function serverOptions(context) {
    const config = vscode.workspace.getConfiguration("mcdpl");
    const configuredCommand = stringSetting(config, "server.command");
    const configuredCwd = stringSetting(config, "server.cwd");
    const configuredArgs = arraySetting(config, "server.args");
    const workspaceRoot = workspaceRootFor(context, configuredCwd);

    if (configuredCommand) {
        return executable(configuredCommand, configuredArgs, workspaceRoot);
    }

    const bundledServer = bundledServerPath(context);
    if (bundledServer) {
        return executable(bundledServer, configuredArgs, workspaceRoot);
    }

    const builtServer = builtServerPath(workspaceRoot);
    if (builtServer && fs.existsSync(builtServer)) {
        return executable(builtServer, configuredArgs, workspaceRoot);
    }

    return executable(
        "cargo",
        ["run", "-p", "mcdp-lsp", "--", ...configuredArgs],
        workspaceRoot,
    );
}

function executable(command, args, cwd) {
    return {
        command,
        args,
        options: { cwd },
        transport: TransportKind.stdio,
    };
}

function workspaceRootFor(context, configuredCwd) {
    if (configuredCwd) {
        return configuredCwd;
    }

    return (
        findRepositoryRootFromWorkspace() ||
        findRepositoryRoot(context.extensionPath) ||
        firstWorkspaceFolder() ||
        context.extensionPath
    );
}

function findRepositoryRootFromWorkspace() {
    for (const folder of vscode.workspace.workspaceFolders || []) {
        const root = findRepositoryRoot(folder.uri.fsPath);
        if (root) {
            return root;
        }
    }

    return undefined;
}

function findRepositoryRoot(start) {
    let current = path.resolve(start);
    let previous = "";

    while (current !== previous) {
        if (
            fs.existsSync(path.join(current, "Cargo.toml")) &&
            fs.existsSync(path.join(current, "crates", "mcdp-lsp", "Cargo.toml"))
        ) {
            return current;
        }

        previous = current;
        current = path.dirname(current);
    }

    return undefined;
}

function firstWorkspaceFolder() {
    return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}

function builtServerPath(workspaceRoot) {
    if (!workspaceRoot) {
        return undefined;
    }

    return path.join(workspaceRoot, "target", "debug", serverBinaryName());
}

function bundledServerPath(context) {
    const serverPath = path.join(
        context.extensionPath,
        "server",
        `${process.platform}-${process.arch}`,
        serverBinaryName(),
    );

    return fs.existsSync(serverPath) ? serverPath : undefined;
}

function serverBinaryName() {
    return process.platform === "win32" ? "mcdp-lsp.exe" : "mcdp-lsp";
}

function stringSetting(config, key) {
    const value = config.get(key, "");
    return typeof value === "string" ? value.trim() : "";
}

function arraySetting(config, key) {
    const value = config.get(key, []);
    if (!Array.isArray(value)) {
        return [];
    }

    return value.filter((item) => typeof item === "string");
}

module.exports = {
    activate,
    deactivate,
};
