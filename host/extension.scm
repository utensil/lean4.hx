;; Load this file from a Helix init.scm after installing the cdylib into
;; STEEL_HOME/native, then call (lean4-hx-install!).
(require-builtin helix/core/editor as editor.)
(require-builtin helix/core/misc as hx.)
(require-builtin helix/core/static as static.)
(require-builtin helix/components as components.)
(#%require-dylib "liblean4_hx"
  (only-in native-state native-activate! native-deactivate! native-active?
           native-record-selection! native-record-insert! native-record-open!
           native-record-close! native-record-request! native-record-callback!
           native-begin-request! native-cancel-request! native-label native-summary
           native-file-uri))

(provide lean4-hx-install!)
(provide lean4-hx-remove-component!)
(provide lean4-hx-request-lean-info!)
(provide lean4-hx-summary)
(provide lean4-hx-native)

;; The value is opaque to Scheme. All counters and lifecycle state are Rust-owned.
(define lean4-hx-native (native-state))
(define lean4-hx-component #f)
(define lean4-hx-installed? #f)
(define lean4-hx-selection-pending? #f)

(define (lean4-hx-summary)
  (native-summary lean4-hx-native))

(define (lean4-hx-status prefix)
  (hx.set-status! (string-append prefix ": " (lean4-hx-summary))))

(define (lean4-hx-on-selection view)
  (native-record-selection! lean4-hx-native)
  (if (and (native-active? lean4-hx-native)
           (not lean4-hx-selection-pending?))
      (begin
        (set! lean4-hx-selection-pending? #t)
        (hx.enqueue-thread-local-callback-with-delay
         250
         (lambda ()
           (set! lean4-hx-selection-pending? #f)
           (if (and (native-active? lean4-hx-native)
                    (> (static.get-current-line-number) 0))
               (begin
                 (native-cancel-request! lean4-hx-native)
                 (lean4-hx-request-lean-info!))
               #f))))
      #f)
  (lean4-hx-status "lean4.hx selection"))

(define (lean4-hx-on-insert character)
  (native-record-insert! lean4-hx-native))

(define (lean4-hx-on-open document)
  (native-record-open! lean4-hx-native)
  ;; LSP startup is asynchronous. Yield to Helix before issuing the first
  ;; request so the language server has received didOpen for this document.
  (hx.enqueue-thread-local-callback-with-delay
   10000
   (lambda ()
     (if (native-active? lean4-hx-native)
         (lean4-hx-request-lean-info!)
         #f))))

(define (lean4-hx-on-close closed-event)
  (native-record-close! lean4-hx-native))

(define (lean4-hx-on-lean-info generation result)
  (if (native-active? lean4-hx-native)
      (begin
        (native-record-callback! lean4-hx-native generation
                                 (value->jsexpr-string result))
        (lean4-hx-status "lean4.hx Lean callback"))
      #f))

(define (lean4-hx-request-lean-info!)
  (let ([path (static.cx->current-file)])
    (if path
        (let* ([line (static.get-current-line-number)]
               [character (static.get-current-line-character "utf-16")]
               [uri (native-file-uri path)]
               [params (hash "textDocument" (hash "uri" uri)
                             "position" (hash "line" line
                                               "character" character))])
          (native-record-request! lean4-hx-native path line character)
          (let ([generation (native-begin-request! lean4-hx-native path line character)])
            (if (> generation 0)
                (hx.send-lsp-command "lean" "$/lean/plainGoal" params
                                     (lambda (result)
                                       (lean4-hx-on-lean-info generation result)))
                #f))
          (lean4-hx-status "lean4.hx Lean request"))
        (hx.set-warning! "lean4.hx: current document has no file URI"))))

(define (lean4-hx-render state area frame)
  (components.frame-set-string! frame
                               (components.area-x area)
                               (components.area-y area)
                               (native-label state)
                               (components.style)))

(define (lean4-hx-handle-event state event)
  components.event-result/ignore)

(define (lean4-hx-install!)
  (if lean4-hx-installed?
      "lean4.hx already installed"
      (begin
        ;; Hooks are generation-scoped by Helix. Removing by name also clears
        ;; a component left behind by a previous init.scm reload.
        (hx.pop-last-component-by-name! "lean4-hx-component")
        (set! lean4-hx-selection-pending? #f)
        (native-activate! lean4-hx-native)
        (editor.register-hook 'selection-did-change lean4-hx-on-selection)
        (editor.register-hook 'post-insert-char lean4-hx-on-insert)
        (editor.register-hook 'document-opened lean4-hx-on-open)
        (editor.register-hook 'document-closed lean4-hx-on-close)
        (set! lean4-hx-component
              (components.new-component! "lean4-hx-component" lean4-hx-native
                                          lean4-hx-render
                                          (hash "handle_event"
                                                lean4-hx-handle-event)))
        (hx.push-component! lean4-hx-component)
        (set! lean4-hx-installed? #t)
        (lean4-hx-status "lean4.hx installed")
        "lean4.hx installed")))

(define (lean4-hx-remove-component!)
  (if lean4-hx-component
      (begin
        (hx.pop-last-component-by-name! "lean4-hx-component")
        (set! lean4-hx-component #f))
      #f)
  (native-deactivate! lean4-hx-native)
  (set! lean4-hx-installed? #f)
  (set! lean4-hx-selection-pending? #f)
  (lean4-hx-status "lean4.hx removed")
  "lean4.hx component removed")
