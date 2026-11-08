import * as vscode from "vscode";
import { affectsSetting, setting } from "./settings";

type ServerStatus =
	| { state: "stopped" }
	| { state: "starting" }
	| { state: "ready"; version: string | undefined }
	| { state: "error"; message: string };

export class StatusBar implements vscode.Disposable {
	private readonly item = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
	private readonly settingsListener: vscode.Disposable;
	private status: ServerStatus = { state: "stopped" };

	public constructor() {
		this.item.name = "Luck";
		this.settingsListener = vscode.workspace.onDidChangeConfiguration((event) => {
			if (affectsSetting(event, "statusBar.enable") || affectsSetting(event, "statusBar.click")) {
				this.render();
			}
		});
		this.render();
	}

	public update(status: ServerStatus): void {
		this.status = status;
		this.render();
	}

	public dispose(): void {
		this.settingsListener.dispose();
		this.item.dispose();
	}

	private render(): void {
		if (!setting("statusBar.enable")) {
			this.item.hide();
			return;
		}
		switch (this.status.state) {
			case "stopped": {
				this.item.text = "$(circle-slash) Luck";
				this.item.tooltip = "Luck language server stopped";
				break;
			}
			case "starting": {
				this.item.text = "$(sync~spin) Luck";
				this.item.tooltip = "Luck language server starting...";
				break;
			}
			case "ready": {
				this.item.text = `$(check) Luck${this.status.version ? ` ${this.status.version}` : ""}`;
				this.item.tooltip = "Luck language server ready";
				break;
			}
			case "error": {
				this.item.text = "$(error) Luck";
				this.item.tooltip = `Luck: ${this.status.message}`;
				break;
			}
		}
		this.item.backgroundColor =
			this.status.state === "error"
				? new vscode.ThemeColor("statusBarItem.errorBackground")
				: undefined;
		this.item.command =
			setting("statusBar.click") === "restart" ? "luck.restart" : "luck.showServerOutput";
		this.item.show();
	}
}
