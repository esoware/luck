import * as vscode from "vscode";
import path from "node:path";
import * as fs from "node:fs";
import type { LuckClient } from "./client";
import { downloadServer } from "./discover";
import { outputChannel } from "./output";

export function registerCommands(
	client: LuckClient,
	extensionVersion: string | undefined,
): vscode.Disposable[] {
	const handlers = {
		"luck.restart": () => client.restart(),
		"luck.start": () => client.start(),
		"luck.stop": () => client.stop(),
		"luck.showServerOutput": () => outputChannel("server").show(true),
		"luck.showExtensionOutput": () => outputChannel("extension").show(true),
		"luck.showTraceOutput": () => outputChannel("trace").show(true),
		"luck.serverVersion": () => showServerVersion(client),
		"luck.formatDocument": () => vscode.commands.executeCommand("editor.action.formatDocument"),
		"luck.formatSelection": () => vscode.commands.executeCommand("editor.action.formatSelection"),
		"luck.applyAllFixesFile": applyAllFixesFile,
		"luck.applyAllFixesWorkspace": () => applyAllFixesWorkspace(client),
		"luck.createConfig": createConfig,
		"luck.downloadServer": () => downloadServer(extensionVersion),
		"luck.viewSyntaxTree": () => viewSyntaxTree(client),
		"luck.copyDebugInfo": () => copyDebugInfo(client, extensionVersion),
		"luck.reportIssue": async () => {
			await vscode.env.openExternal(vscode.Uri.parse("https://github.com/esoware/luck/issues/new"));
		},
		"luck.openWalkthrough": () =>
			vscode.commands.executeCommand(
				"workbench.action.openWalkthrough",
				"luck.luck#luck.gettingStarted",
				false,
			),
	} satisfies Record<string, () => unknown>;

	return Object.entries(handlers).map(([command, handler]) =>
		vscode.commands.registerCommand(command, handler),
	);
}

function showServerVersion(client: LuckClient): void {
	const serverInfo = client.languageClient?.initializeResult?.serverInfo;
	void vscode.window.showInformationMessage(
		`${serverInfo?.name ?? "luck"} ${serverInfo?.version ?? "unknown"}`,
	);
}

async function applyAllFixesFile(): Promise<void> {
	if (!vscode.window.activeTextEditor) {
		return;
	}
	await vscode.commands.executeCommand("editor.action.sourceAction", {
		kind: "source.fixAll.luck",
		apply: "first",
	});
}

async function applyAllFixesWorkspace(client: LuckClient): Promise<void> {
	const { languageClient } = client;
	if (!languageClient) {
		void vscode.window.showWarningMessage("Luck: server is not running.");
		return;
	}
	try {
		await languageClient.sendRequest("luck/fixAllWorkspace", {});
		void vscode.window.showInformationMessage("Luck: workspace fixes applied.");
	} catch (error) {
		void vscode.window.showErrorMessage(`Luck: ${error}`);
	}
}

async function createConfig(): Promise<void> {
	const folder = vscode.workspace.workspaceFolders?.[0];
	if (!folder) {
		void vscode.window.showErrorMessage("Luck: open a folder before creating a config.");
		return;
	}
	const configPath = path.join(folder.uri.fsPath, "luck.json");
	if (fs.existsSync(configPath)) {
		void vscode.window.showWarningMessage("Luck: luck.json already exists.");
	} else {
		const skeleton = {
			lua: "lua54",
			luau: "luau",
			format: {
				line_width: 100,
				indent_style: "tabs",
				indent_width: 4,
				quote_style: "double",
			},
			lint: {
				extra_globals: [],
			},
		};
		await fs.promises.writeFile(configPath, `${JSON.stringify(skeleton, null, 2)}\n`, "utf8");
	}
	await vscode.window.showTextDocument(vscode.Uri.file(configPath));
}

async function viewSyntaxTree(client: LuckClient): Promise<void> {
	const { languageClient } = client;
	const editor = vscode.window.activeTextEditor;
	if (!languageClient || !editor) {
		void vscode.window.showWarningMessage("Luck: open a Lua file first.");
		return;
	}
	try {
		const tree = await languageClient.sendRequest<string>("luck/syntaxTree", {
			textDocument: { uri: editor.document.uri.toString() },
		});
		const document = await vscode.workspace.openTextDocument({
			content: tree,
			language: "luck-syntax-tree",
		});
		await vscode.window.showTextDocument(document, vscode.ViewColumn.Beside);
	} catch (error) {
		void vscode.window.showErrorMessage(`Luck: ${error}`);
	}
}

async function copyDebugInfo(
	client: LuckClient,
	extensionVersion: string | undefined,
): Promise<void> {
	const serverVersion = client.languageClient?.initializeResult?.serverInfo?.version;
	const lines = [
		`Luck extension: ${extensionVersion ?? "unknown"}`,
		`Luck server:    ${serverVersion ?? "unknown"}`,
		`VS Code:        ${vscode.version}`,
		`Platform:       ${process.platform}/${process.arch}`,
		`Node:           ${process.version}`,
	];
	await vscode.env.clipboard.writeText(lines.join("\n"));
	void vscode.window.showInformationMessage("Luck: debug info copied.");
}
