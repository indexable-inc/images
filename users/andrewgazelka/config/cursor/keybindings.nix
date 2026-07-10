[
  {
    command = "-workbench.action.terminal.openNativeConsole";
    key = "shift+cmd+c";
  }
  {
    command = "-editor.emmet.action.nextEditPoint";
    key = "ctrl+n";
  }
  {
    command = "-extension.changeCase.next";
    key = "ctrl+n";
  }
  {
    command = "editor.action.addSelectionToPreviousFindMatch";
    key = "alt+shift+n";
    when = "editorFocus";
  }
  {
    command = "editor.action.moveSelectionToNextFindMatch";
    key = "alt+d";
    when = "editorFocus";
  }
  {
    command = "gitlens.copyRemoteFileUrlToClipboard";
    key = "shift+cmd+c";
    when = "editorTextFocus";
  }
  {
    command = "redo";
    key = "ctrl+r";
    when = "editorTextFocus";
  }
  {
    command = "-workbench.action.toggleSidebarVisibility";
    key = "cmd+b";
  }
  {
    command = "-mdx.toggleStrong";
    key = "cmd+b";
    when = "editorTextFocus && !editorReadonly && editorLangId == 'mdx'";
  }
  {
    command = "composerMode.plan";
    key = "ctrl+cmd+p";
  }
  {
    command = "-composer.showBackgroundAgentHistory";
    key = "cmd+e";
    when = "backgroundComposerEnabled || showBackgroundAgentHistoryAction";
  }
  {
    command = "-workbench.action.files.saveAs";
    key = "shift+cmd+s";
  }
  {
    command = "-workbench.action.backgroundComposer.toggleSidebar";
    key = "shift+cmd+s";
    when = "backgroundComposerEnabled";
  }
  {
    command = "-workbench.action.files.saveLocalFile";
    key = "shift+cmd+s";
    when = "remoteFileDialogVisible";
  }
  {
    command = "workbench.action.gotoSymbol";
    key = "shift+cmd+s";
    when = "editorTextFocus";
  }
  {
    command = "-editor.action.indentLines";
    key = "cmd+]";
    when = "editorTextFocus && !editorReadonly";
  }
  {
    command = "-workbench.action.debug.run";
    key = "ctrl+d";
    when = "!inDebugMode && !terminalFocus";
  }
  {
    command = "-workbench.action.debug.run";
    key = "ctrl+d";
    when = "debuggersAvailable && !inDebugMode && !terminalFocus";
  }
  {
    command = "-deleteRight";
    key = "ctrl+d";
    when = "textInputFocus";
  }
  {
    command = "editor.action.goToTypeDefinition";
    key = "shift+cmd+b";
    when = "editorHasTypeDefinitionProvider && editorTextFocus";
  }
  {
    command = "-git.pushTo";
    key = "alt+cmd+k";
    when = "!inDebugMode && !operationInProgress && !terminalFocus";
  }
  {
    command = "-keybindings.editor.recordSearchKeys";
    key = "alt+cmd+k";
    when = "inKeybindings && inKeybindingsSearch";
  }
  {
    command = "editor.action.showHover";
    key = "ctrl+j";
    when = "editorTextFocus";
  }
  {
    command = "selectNextSuggestion";
    key = "ctrl+n";
    when = "suggestWidgetMultipleSuggestions && suggestWidgetVisible && textInputFocus";
  }
  {
    command = "-git.commitAll";
    key = "cmd+k";
    when = "!inDebugMode && !operationInProgress && !terminalFocus";
  }
  {
    command = "-git.revertSelectedRanges";
    key = "cmd+k cmd+r";
    when = "editorTextFocus && !operationInProgress && resourceScheme == 'file'";
  }
  {
    command = "-git.stageSelectedRanges";
    key = "cmd+k alt+cmd+s";
    when = "editorTextFocus && !operationInProgress && resourceScheme == 'file'";
  }
  {
    command = "-git.unstageSelectedRanges";
    key = "cmd+k cmd+n";
    when = "editorTextFocus && isInDiffEditor && isInDiffRightEditor && !operationInProgress && resourceScheme == 'git'";
  }
  {
    command = "-markdown.showPreviewToSide";
    key = "cmd+k v";
    when = "!notebookEditorFocused && editorLangId == 'markdown'";
  }
  {
    command = "-notebook.cell.changeLanguage";
    key = "cmd+k m";
    when = "notebookCellEditable && notebookEditable && notebookEditorFocused";
  }
  {
    command = "-cursorai.action.generateInTerminal";
    key = "cmd+k";
    when = "terminalFocus && terminalHasBeenCreated || terminalFocus && terminalProcessSupported || terminalHasBeenCreated && terminalPromptBarVisible || terminalProcessSupported && terminalPromptBarVisible";
  }
  {
    command = "-typst-preview.preview";
    key = "cmd+k v";
    when = "editorLangId == 'typst'";
  }
  {
    command = "-editor.action.inlineDiffs.focusEditor";
    key = "cmd+k";
    when = "editorHasPromptBar && editorPromptBarFocused";
  }
  {
    command = "-workbench.action.showHover";
    key = "cmd+k cmd+i";
    when = "!editorTextFocus";
  }
  {
    command = "-workbench.debug.panel.action.clearReplAction";
    key = "cmd+k";
    when = "focusedView == 'workbench.panel.repl.view'";
  }
  {
    command = "selectNextCodeAction";
    key = "ctrl+n";
    when = "codeActionMenuVisible";
  }
  {
    command = "selectPrevCodeAction";
    key = "ctrl+p";
    when = "codeActionMenuVisible";
  }
  {
    command = "editor.action.marker.nextInFiles";
    key = "alt+j";
    when = "editorFocus";
  }
  {
    command = "editor.action.marker.prevInFiles";
    key = "alt+k";
    when = "editorFocus";
  }
  {
    command = "list.focusDown";
    key = "alt+j";
    when = "problemsViewFocus";
  }
  {
    command = "list.focusUp";
    key = "alt+k";
    when = "problemsViewFocus";
  }
  {
    args = {text = "\\\r\n";};
    command = "workbench.action.terminal.sendSequence";
    key = "shift+enter";
    when = "terminalFocus";
  }
  {
    command = "-workbench.action.quickOpenSelectNext";
    key = "ctrl+n";
    when = "inQuickOpen";
  }
  {
    command = "-workbench.action.debug.prevConsole";
    key = "shift+cmd+[";
    when = "inDebugRepl";
  }
  {
    command = "-workbench.action.terminal.focusPrevious";
    key = "shift+cmd+[";
    when = "terminalHasBeenCreated || terminalProcessSupported";
  }
  {
    command = "-workbench.action.terminal.focusPrevious";
    key = "shift+cmd+[";
    when = "terminalFocus && terminalHasBeenCreated && !terminalEditorFocus || terminalFocus && terminalProcessSupported && !terminalEditorFocus";
  }
  {
    command = "-workbench.action.debug.nextConsole";
    key = "shift+cmd+]";
    when = "inDebugRepl";
  }
  {
    command = "-workbench.action.terminal.focusNext";
    key = "shift+cmd+]";
    when = "terminalFocus && terminalHasBeenCreated && !terminalEditorFocus || terminalFocus && terminalProcessSupported && !terminalEditorFocus";
  }
  {
    command = "-workbench.action.terminal.focusNext";
    key = "shift+cmd+]";
    when = "terminalHasBeenCreated || terminalProcessSupported";
  }
  {
    command = "-workbench.action.files.newUntitledFile";
    key = "cmd+n";
  }
  {
    command = "-editor.action.inlineDiffs.rejectPartialEdit";
    key = "cmd+n";
    when = "editorTextFocus && inlineDiffs.activeEditorWithDiffs";
  }
  {
    command = "-editor.action.sourceAction";
    key = "cmd+n";
    when = "editorHasCodeActionsProvider && editorTextFocus && !editorReadonly";
  }
  {
    command = "workbench.action.focusActiveEditorGroup";
    key = "escape";
    when = "sideBarFocus";
  }
  {
    command = "rust-analyzer.parentModule";
    key = "cmd+u";
    when = "editorTextFocus && editorLangId == 'rust'";
  }
  {
    command = "composerMode.agent";
    key = "cmd+i";
  }
  {
    command = "gitlens.toggleFileBlame";
    key = "alt+b";
    when = "editorTextFocus";
  }
]
