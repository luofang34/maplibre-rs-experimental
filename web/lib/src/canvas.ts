export const preventDefaultTouchActions = () => {
    document.body.querySelectorAll("canvas").forEach(canvas => {
        canvas.style.touchAction = "none"
        canvas.addEventListener("touchstart", e => e.preventDefault())
        canvas.addEventListener("touchmove", e => e.preventDefault())
        canvas.addEventListener("contextmenu", e => e.preventDefault())
        canvas.addEventListener("mousedown", e => {
            if (e.button === 1) {
                e.preventDefault()
            }
        })
        canvas.addEventListener("auxclick", e => {
            if (e.button === 1) {
                e.preventDefault()
            }
        })
    })
}
