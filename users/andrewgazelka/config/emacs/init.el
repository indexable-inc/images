;;; init.el --- Emacs configuration -*- lexical-binding: t -*-

;; ============================================
;; Basic UI
;; ============================================
(setq inhibit-startup-message t)
(menu-bar-mode -1)
(tool-bar-mode -1)
(scroll-bar-mode -1)
(global-display-line-numbers-mode 1)
(setq display-line-numbers-type 'relative)
(column-number-mode 1)
(show-paren-mode 1)
(global-hl-line-mode 1)

;; Font with Nerd Font icons (installed via nix: nerd-fonts.fira-code)
(when (display-graphic-p)
  (set-face-attribute 'default nil :family "FiraCode Nerd Font Mono" :height 140))

;; ============================================
;; Sane defaults
;; ============================================
(setq-default indent-tabs-mode nil)
(setq-default tab-width 4)
(setq make-backup-files nil)
(setq auto-save-default nil)
(setq create-lockfiles nil)
(setq ring-bell-function 'ignore)
(setq use-short-answers t)
(defalias 'yes-or-no-p 'y-or-n-p)
(global-auto-revert-mode 1)
(setq auto-revert-verbose nil)
(setq global-auto-revert-non-file-buffers t)

;; macOS: use Option as Meta, Command as Super
(when (eq system-type 'darwin)
  (setq mac-option-modifier 'meta)
  (setq mac-command-modifier 'super))

;; ============================================
;; Package management
;; ============================================
(require 'package)
(setq package-archives
      '(("melpa" . "https://melpa.org/packages/")
        ("gnu"   . "https://elpa.gnu.org/packages/")
        ("nongnu" . "https://elpa.nongnu.org/nongnu/")))
(unless package-archive-contents
  (package-refresh-contents))

(unless (package-installed-p 'use-package)
  (package-install 'use-package))
(require 'use-package)
(setq use-package-always-ensure t)

;; ============================================
;; Evil mode (vim emulation)
;; ============================================
(use-package evil
  :init
  (setq evil-want-integration t)
  (setq evil-want-keybinding nil)
  (setq evil-want-C-u-scroll t)
  (setq evil-want-C-i-jump t)
  (setq evil-undo-system 'undo-redo)
  (setq evil-search-module 'evil-search)
  (setq evil-split-window-below t)
  (setq evil-vsplit-window-right t)
  :config
  (evil-mode 1))

(use-package evil-collection
  :after evil
  :config
  (evil-collection-init))

(use-package evil-commentary
  :after evil
  :config
  (evil-commentary-mode))

(use-package evil-surround
  :after evil
  :config
  (global-evil-surround-mode 1))

;; ============================================
;; Leader key (space)
;; ============================================
(use-package general
  :after evil
  :config
  (general-create-definer my/leader
    :states '(normal visual motion)
    :keymaps 'override
    :prefix "SPC")
  (my/leader
    "f"  '(:ignore t :which-key "files")
    "ff" 'find-file
    "fs" 'save-buffer
    "fr" 'consult-recent-file
    "b"  '(:ignore t :which-key "buffers")
    "bb" 'consult-buffer
    "bk" 'kill-current-buffer
    "w"  '(:ignore t :which-key "windows")
    "wv" 'split-window-right
    "ws" 'split-window-below
    "wd" 'delete-window
    "wh" 'evil-window-left
    "wj" 'evil-window-down
    "wk" 'evil-window-up
    "wl" 'evil-window-right
    "p"  '(:ignore t :which-key "project")
    "pp" 'project-switch-project
    "pf" 'project-find-file
    "/"  'consult-ripgrep
    "SPC" 'project-find-file))

(use-package which-key
  :init (which-key-mode)
  :config (setq which-key-idle-delay 0.3))

;; ============================================
;; Completion stack: vertico + consult + marginalia + orderless
;; ============================================
(use-package vertico
  :init (vertico-mode))

(use-package savehist
  :ensure nil
  :init (savehist-mode))

(use-package marginalia
  :init (marginalia-mode))

(use-package orderless
  :init
  (setq completion-styles '(orderless basic)
        completion-category-defaults nil
        completion-category-overrides '((file (styles partial-completion)))))

(use-package consult)

;; ============================================
;; Editing helpers
;; ============================================
(use-package corfu
  :init (global-corfu-mode)
  :config
  (setq corfu-auto t
        corfu-auto-prefix 2
        corfu-cycle t))

(use-package magit
  :commands magit-status)

(use-package rainbow-delimiters
  :hook (prog-mode . rainbow-delimiters-mode))

;; ============================================
;; Theme
;; ============================================
(use-package doom-themes
  :config
  (load-theme 'doom-one t))

(use-package doom-modeline
  :init (doom-modeline-mode 1))

;;; init.el ends here
