// Correctness-focused lint setup: Vue "essential" rules plus the recommended
// JS/TS rules. Formatting is handled by Prettier, not ESLint.
import js from '@eslint/js';
import pluginVue from 'eslint-plugin-vue';
import prettierConfig from 'eslint-config-prettier';
import globals from 'globals';
import tseslint from 'typescript-eslint';

export default tseslint.config(
    {
        ignores: ['dist/**', 'src-tauri/**', 'node_modules/**', '.pnpm-store/**', 'public/**'],
    },
    js.configs.recommended,
    ...tseslint.configs.recommended,
    ...pluginVue.configs['flat/essential'],
    {
        files: ['**/*.vue'],
        languageOptions: {
            parserOptions: { parser: tseslint.parser },
        },
    },
    {
        languageOptions: {
            ecmaVersion: 'latest',
            sourceType: 'module',
            globals: { ...globals.browser },
        },
        rules: {
            // The app names its views and panels with single words (Home, Editor).
            'vue/multi-word-component-names': 'off',
            // Mirrors tsconfig's noUnusedLocals/noUnusedParameters, which already
            // allow `_`-prefixed names.
            '@typescript-eslint/no-unused-vars': [
                'error',
                { argsIgnorePattern: '^_', varsIgnorePattern: '^_', caughtErrors: 'none' },
            ],
        },
    },
    {
        files: ['scripts/**', '*.config.{js,ts}'],
        languageOptions: { globals: { ...globals.node } },
    },
    // Formatting is Prettier's job; switch off any stylistic rules it would fight.
    prettierConfig,
);
