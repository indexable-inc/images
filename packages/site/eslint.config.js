import js from '@eslint/js';
import svelte from 'eslint-plugin-svelte';
import globals from 'globals';
import tseslint from 'typescript-eslint';

const typedFiles = ['src/**/*.{ts,svelte}'];

export default tseslint.config(
  {
    ignores: ['build', '.svelte-kit']
  },
  js.configs.recommended,
  ...tseslint.configs.strictTypeChecked.map((config) => ({
    ...config,
    files: typedFiles
  })),
  ...svelte.configs['flat/recommended'],
  {
    files: typedFiles,
    languageOptions: {
      parserOptions: {
        extraFileExtensions: ['.svelte'],
        parser: tseslint.parser,
        projectService: true,
        tsconfigRootDir: import.meta.dirname
      }
    }
  },
  {
    files: typedFiles,
    languageOptions: {
      globals: {
        ...globals.browser
      }
    }
  },
  {
    files: ['*.config.{js,ts}', 'svelte.config.js'],
    languageOptions: {
      globals: {
        ...globals.node
      }
    }
  },
  {
    files: typedFiles,
    rules: {
      '@typescript-eslint/no-explicit-any': 'error'
    }
  }
);
