;; Load this file from a Helix init.scm after installing the cdylib into
;; STEEL_HOME/native.
(require-builtin helix/core/editor as editor.)
(require-builtin helix/core/misc as hx.)
(require-builtin helix/components as components.)
(#%require-dylib "liblean4_hx"
  (only-in native-state native-label))

(provide lean4-hx-install!)
(provide lean4-hx-request-lean-info!)
(provide lean4-hx-remove-component!)
(provide lean4-hx-selection-count)
(provide lean4-hx-insert-count)
(provide lean4-hx-callback-count)

(define lean4-hx-selection-count 0)
(define lean4-hx-insert-count 0)
(define lean4-hx-callback-count 0)

(define (lean4-hx-on-selection view)
  (set! lean4-hx-selection-count (+ lean4-hx-selection-count 1))
  (hx.set-status! (string-append "lean4.hx selection hook (view " (number->string view) ")")))

(define (lean4-hx-on-lean-info result)
  (set! lean4-hx-callback-count (+ lean4-hx-callback-count 1))
  (hx.set-status! (string-append "lean4.hx Lean callback: " (if result "reply" "no reply"))))

(define (lean4-hx-request-lean-info!)
  (hx.send-lsp-command "lean" "$/lean/plainGoal" (hash) lean4-hx-on-lean-info))

(define lean4-hx-component #f)
(define lean4-hx-native (native-state))

(define (lean4-hx-render state area frame)
  (components.frame-set-string! frame
                               (components.area-x area)
                               (components.area-y area)
                               (native-label state)
                               (components.style)))

(define (lean4-hx-handle-event state event)
  (components.event-result/ignore))

(define (lean4-hx-install!)
  (editor.register-hook 'selection-did-change lean4-hx-on-selection)
  (editor.register-hook 'post-insert-char
                        (lambda (character)
                          (set! lean4-hx-insert-count (+ lean4-hx-insert-count 1))))
  (set! lean4-hx-component
        (components.new-component! "lean4-hx-component" lean4-hx-native lean4-hx-render
                                    (hash "handle_event" lean4-hx-handle-event)))
  (hx.push-component! lean4-hx-component)
  "lean4.hx installed")

(define (lean4-hx-remove-component!)
  (hx.pop-last-component-by-name! "lean4-hx-component")
  (set! lean4-hx-component #f)
  "lean4.hx component removed")
