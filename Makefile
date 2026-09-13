.PHONY: help doctor install-deps version version-check dev preview build build-debug package appimage release release-dry-run release-notes test test-release test-dev test-integration test-ui test-native test-startup test-storage lint format typecheck quality clean

help: ## Show available commands
	@python3 scripts/doctor.py --help-targets

doctor: ## Check the local development environment
	python3 scripts/doctor.py

install-deps: ## Install locked frontend dependencies
	pnpm install --frozen-lockfile

version: ## Synchronize application and package metadata from VERSION
	node scripts/version.mjs sync

version-check: ## Verify that committed package metadata matches VERSION
	node scripts/version.mjs check

dev: ## Launch the native desktop app in development mode
	pnpm desktop

preview: ## Open a frontend-only development server
	pnpm dev

build: ## Build the release desktop executable
	pnpm tauri build --no-bundle

build-debug: ## Build a standalone debug desktop executable
	pnpm tauri build --debug --no-bundle

package: ## Build native installers (Linux: Debian package and AppImage)
	pnpm tauri build -- --locked

package appimage: export APPIMAGE_EXTRACT_AND_RUN = 1

appimage: ## Build the Linux AppImage
	pnpm tauri build --bundles appimage -- --locked

release: ## Tag committed VERSION and push the tag to trigger GitHub Release
	node scripts/release.mjs publish

release-dry-run: ## Validate and preview publication without creating or pushing a tag
	node scripts/release.mjs publish --dry-run

release-notes: ## Print release notes with commit links
	node scripts/release.mjs notes

test: ## Run core contract and property tests
	cargo test --workspace --locked

test-release: ## Test version synchronization and release publication safeguards
	pnpm test:release

test-dev: ## Test automatic development ports and process cleanup
	pnpm test:dev

test-startup: ## Check the development WebView with ambient proxy variables (Linux WebDriver required)
	cargo build -p proxy-vouch --locked
	python3 scripts/proxy_environment_smoke.py

test-integration: ## Check real protocols against local proxy fixtures
	cargo build -p proxy-vouch-core --example check --locked
	python3 scripts/network_fixtures.py

test-ui: ## Run browser layout and accessibility smoke checks
	python3 scripts/run_ui_tests.py

test-native: ## Run the real Tauri GUI smoke (requires a running tauri-driver)
	python3 scripts/native_smoke.py

test-storage: ## Verify native restart, autosave and backup dialogs in isolated Linux user folders
	pnpm tauri build --debug --no-bundle -- --locked
	python3 scripts/storage_smoke.py

lint: ## Check Rust and TypeScript without warnings
	cargo clippy --workspace --all-targets --locked -- -D warnings
	pnpm typecheck

format: ## Format Rust and frontend sources
	cargo fmt --all
	pnpm exec prettier --write src tests-ui scripts/*.mjs scripts/tests/*.mjs *.json *.ts index.html src-tauri/*.json src-tauri/capabilities/*.json

typecheck: ## Check frontend types
	pnpm typecheck

quality: ## Run format, static, contract and protocol checks
	pnpm format:check
	$(MAKE) version-check lint test test-release test-dev test-integration test-ui

clean: ## Remove only generated build and test output
	cargo clean
	python3 scripts/clean.py
