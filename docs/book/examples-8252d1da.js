// Tab switcher for mdBook example pages (Rust / JavaScript / Full HTML).
(function () {
    const TAB_LABELS = ['Rust', 'JavaScript', 'Full HTML'];

    function initExampleCodeTabs() {
        const containers = document.querySelectorAll('.example-code-tabs');
        containers.forEach((container) => {
            if (container.classList.contains('tabs-initialized')) {
                return;
            }
            const blocks = Array.from(container.querySelectorAll(':scope > pre'));
            if (blocks.length === 0) {
                return;
            }

            const tabBar = document.createElement('div');
            tabBar.className = 'example-tab-bar';
            tabBar.setAttribute('role', 'tablist');

            const buttons = blocks.map((block, index) => {
                const button = document.createElement('button');
                button.type = 'button';
                button.className = 'example-tab-button';
                button.setAttribute('role', 'tab');
                button.textContent = TAB_LABELS[index] || `Tab ${index + 1}`;
                button.addEventListener('click', () => selectTab(index));
                tabBar.appendChild(button);
                return button;
            });

            function selectTab(activeIndex) {
                blocks.forEach((block, idx) => {
                    const isActive = idx === activeIndex;
                    block.classList.toggle('active-tab', isActive);
                    buttons[idx].classList.toggle('active', isActive);
                    buttons[idx].setAttribute('aria-selected', isActive ? 'true' : 'false');
                });
            }

            container.insertBefore(tabBar, container.firstChild);
            container.classList.add('tabs-initialized');
            // Default to Rust (index 0)
            selectTab(0);
        });
    }

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', initExampleCodeTabs);
    } else {
        initExampleCodeTabs();
    }
})();
