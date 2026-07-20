import * as vscode from "vscode";

export type ServerStatus =
	| { state: "stopped" }
	| { state: "starting" }
	| { state: "ready"; version?: string | undefined }
	| { state: "error"; message: string };

export class StatusBar implements vscode.Disposable {
	private item: vscode.StatusBarItem;
	private current: ServerStatus = { state: "stopped" };

	public constructor() {
		this.item = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
		this.item.name = "Luck";
		this.render();
	}

	public update(status: ServerStatus): void {
		this.current = status;
		this.render();
	}

	// Re-apply the current status against fresh config, e.g. after
	// `luck.statusBar.*` settings change.
	public refresh(): void {
		this.render();
	}

	public dispose(): void {
		this.item.dispose();
	}

	private render(): void {
		const config = vscode.workspace.getConfiguration("luck");
		if (!config.get<boolean>("statusBar.enable", true)) {
			this.item.hide();
			return;
		}
		switch (this.current.state) {
			case "stopped": {
				this.item.text = "$(circle-slash) Luck";
				this.item.tooltip = "Luck language server stopped";
				this.item.backgroundColor = undefined;
				break;
			}
			case "starting": {
				this.item.text = "$(sync~spin) Luck";
				this.item.tooltip = "Luck language server starting...";
				this.item.backgroundColor = undefined;
				break;
			}
			case "ready": {
				this.item.text = `$(check) Luck${this.current.version ? ` ${this.current.version}` : ""}`;
				this.item.tooltip = "Luck language server ready";
				this.item.backgroundColor = undefined;
				break;
			}
			case "error": {
				this.item.text = "$(error) Luck";
				this.item.tooltip = `Luck: ${this.current.message}`;
				this.item.backgroundColor = new vscode.ThemeColor("statusBarItem.errorBackground");
				break;
			}
		}
		this.item.command = clickCommand(config);
		this.item.show();
	}
}

function clickCommand(config: vscode.WorkspaceConfiguration): string {
	const click = config.get<string>("statusBar.click", "showOutput");
	return click === "restart" ? "luck.restart" : "luck.showServerOutput";
}
