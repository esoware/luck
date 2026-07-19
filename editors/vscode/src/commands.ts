import * as vscode from "vscode";
import * as path from "path";
import * as fs from "fs";
import { LuckClient } from "./client";
import { serverOutput, extensionOutput, traceOutput } from "./output";
import { downloadServer } from "./discover";

export function registerCommands(
	context: vscode.ExtensionContext,
	client: LuckClient,
): void {
	const sub = (id: string, fn: (...args: any[]) => any) =>
		context.subscriptions.push(vscode.commands.registerCommand(id, fn));

	sub("luck.restart", () => client.restart());
	sub("luck.start", () => client.start());
	sub("luck.stop", () => client.stop());

	sub("luck.showServerOutput", () => serverOutput().show(true));
	sub("luck.showExtensionOutput", () => extensionOutput().show(true));
	sub("luck.showTraceOutput", () => traceOutput().show(true));

	sub("luck.serverVersion", async () => {
		const lc = client.languageClient;
		const version = lc?.initializeResult?.serverInfo?.version ?? "unknown";
		const name = lc?.initializeResult?.serverInfo?.name ?? "luck";
		vscode.window.showInformationMessage(`${name} ${version}`);
	});

	sub("luck.formatDocument", async () => {
		await vscode.commands.executeCommand("editor.action.formatDocument");
	});

	sub("luck.formatSelection", async () => {
		await vscode.commands.executeCommand("editor.action.formatSelection");
	});

	sub("luck.applyAllFixesFile", async () => {
		const editor = vscode.window.activeTextEditor;
		if (!editor) return;
		await vscode.commands.executeCommand(
			"editor.action.sourceAction",
			{
				kind: "source.fixAll.luck",
				apply: "first",
			},
		);
	});

	sub("luck.applyAllFixesWorkspace", async () => {
		const lc = client.languageClient;
		if (!lc) {
			vscode.window.showWarningMessage("Luck: server is not running.");
			return;
		}
		try {
			await lc.sendRequest("luck/fixAllWorkspace", {});
			vscode.window.showInformationMessage("Luck: workspace fixes applied.");
		} catch (err) {
			vscode.window.showErrorMessage(`Luck: ${err}`);
		}
	});

	sub("luck.createConfig", async () => {
		const folder = vscode.workspace.workspaceFolders?.[0];
		if (!folder) {
			vscode.window.showErrorMessage(
				"Luck: open a folder before creating a config.",
			);
			return;
		}
		const target = folder.uri.fsPath;
		const file = path.join(target, "luck.json");
		if (fs.existsSync(file)) {
			vscode.window.showWarningMessage("Luck: luck.json already exists.");
			const doc = await vscode.workspace.openTextDocument(file);
			vscode.window.showTextDocument(doc);
			return;
		}
		const skeleton = JSON.stringify(
			{
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
			},
			null,
			2,
		);
		await fs.promises.writeFile(file, skeleton + "\n", "utf8");
		const doc = await vscode.workspace.openTextDocument(file);
		vscode.window.showTextDocument(doc);
	});

	sub("luck.downloadServer", () => downloadServer(context));

	sub("luck.viewSyntaxTree", async () => {
		const lc = client.languageClient;
		const editor = vscode.window.activeTextEditor;
		if (!lc || !editor) {
			vscode.window.showWarningMessage("Luck: open a Lua file first.");
			return;
		}
		try {
			const tree = await lc.sendRequest<string>("luck/syntaxTree", {
				textDocument: { uri: editor.document.uri.toString() },
			});
			const doc = await vscode.workspace.openTextDocument({
				content: tree,
				language: "luck-syntax-tree",
			});
			vscode.window.showTextDocument(doc, vscode.ViewColumn.Beside);
		} catch (err) {
			vscode.window.showErrorMessage(`Luck: ${err}`);
		}
	});

	sub("luck.copyDebugInfo", async () => {
		const lc = client.languageClient;
		const serverVer =
			lc?.initializeResult?.serverInfo?.version ?? "unknown";
		const extVer =
			(context.extension.packageJSON?.version as string | undefined) ??
			"unknown";
		const lines = [
			`Luck extension: ${extVer}`,
			`Luck server:    ${serverVer}`,
			`VS Code:        ${vscode.version}`,
			`Platform:       ${process.platform}/${process.arch}`,
			`Node:           ${process.version}`,
		];
		await vscode.env.clipboard.writeText(lines.join("\n"));
		vscode.window.showInformationMessage("Luck: debug info copied.");
	});

	sub("luck.reportIssue", async () => {
		const repo = "esoware/luck";
		const url = `https://github.com/${repo}/issues/new`;
		await vscode.env.openExternal(vscode.Uri.parse(url));
	});

	sub("luck.openWalkthrough", () =>
		vscode.commands.executeCommand(
			"workbench.action.openWalkthrough",
			"luck.luck#luck.gettingStarted",
			false,
		),
	);
}
