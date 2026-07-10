[
  {
    bindings = {
      ctrl-n = "menu::SelectNext";
      ctrl-p = "menu::SelectPrevious";
    };
  }
  {
    bindings = {
      ctrl-n = "editor::MoveDown";
      ctrl-p = "editor::MoveUp";
    };
    context = "Editor";
  }
  {
    bindings = {
      ctrl-n = "editor::ContextMenuNext";
      ctrl-p = "editor::ContextMenuPrevious";
    };
    context = "Editor && showing_completions";
  }
  {
    bindings = {
      ctrl-j = "editor::Hover";
      ctrl-shift-j = "editor::GoToDefinition";
    };
    context = "Editor";
  }
  {context = "Editor && vim_mode == normal";}
  {
    bindings = {
      cmd-- = null;
    };
    context = "Editor";
  }
  {
    bindings = {
      cmd-p = "editor::ShowSignatureHelp";
    };
    context = "Editor";
  }
  {
    bindings = {
      cmd-1 = "workspace::ToggleLeftDock";
      cmd-e = "file_finder::Toggle";
    };
  }
  {
    bindings = {
      cmd-i = "agent::Toggle";
    };
  }
  {
    bindings = {
      cmd-backspace = [
        "project_panel::Delete"
        {skip_prompt = true;}
      ];
    };
    context = "ProjectPanel";
  }
  {
    bindings = {
      shift-enter = "search::SelectPreviousMatch";
    };
    context = "BufferSearchBar";
  }
  {
    bindings = {
      shift-enter = "search::SelectPreviousMatch";
    };
    context = "BufferSearchBar && !in_replace > Editor";
  }
  {
    bindings = {
      shift-enter = "search::SelectPreviousMatch";
    };
    context = "ProjectSearchBar";
  }
]
