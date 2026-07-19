import * as vscode from "vscode";

export type ServerStatus =
	| { state: "stopped" }
	| { state: "starting" }
	| { state: "ready"; version?: string | undefined }
	| { state: "error"; message: string };

export class StatusBar implements vscode.Disposable {
	private item: vscode.StatusBarItem;

	constructor() {
		this.item = vscode.window.createStatusBarItem(
			vscode.StatusBarAlignment.Left,
			100,
		);
		this.item.name = "Luck";
		this.update({ state: "stopped" });
	}

	update(status: ServerStatus): void {
		const config = vscode.workspace.getConfiguration("luck");
		if (!config.get<boolean>("statusBar.enable", true)) {
			this.item.hide();
			return;
		}
		switch (status.state) {
			case "stopped":
				this.item.text = "$(circle-slash) Luck";
				this.item.tooltip = "Luck language server stopped";
				this.item.backgroundColor = undefined;
				break;
			case "starting":
				this.item.text = "$(sync~spin) Luck";
				this.item.tooltip = "Luck language server starting…";
				this.item.backgroundColor = undefined;
				break;
			case "ready":
				this.item.text = `$(check) Luck${status.version ? ` ${status.version}` : ""}`;
				this.item.tooltip = "Luck language server ready";
				this.item.backgroundColor = undefined;
				break;
			case "error":
				this.item.text = "$(error) Luck";
				this.item.tooltip = `Luck: ${status.message}`;
				this.item.backgroundColor = new vscode.ThemeColor(
					"statusBarItem.errorBackground",
				);
				break;
		}
		this.item.command = clickCommand();
		this.item.show();
	}

	dispose(): void {
		this.item.dispose();
	}
}

function clickCommand(): string {
	const config = vscode.workspace.getConfiguration("luck");
	const click = config.get<string>("statusBar.click", "showOutput");
	switch (click) {
		case "restart":
			return "luck.restart";
		case "showOutput":
		default:
			return "luck.showServerOutput";
	}
}
