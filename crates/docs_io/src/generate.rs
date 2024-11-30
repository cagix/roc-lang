use crate::file::{self, Assets};
use crate::problem::Problem;
use bumpalo::collections::Vec;
use bumpalo::{collections::string::String, Bump};
use core::{fmt::Debug, slice};
use roc_can::scope::Scope;
use roc_collections::VecSet;
use roc_docs_render::{
    AbilityImpl, AbilityMember, BodyEntry, Docs, SidebarEntry, TypeAnn, TypeRenderer,
};
use roc_load::docs::{DocEntry, TypeAnnotation};
use roc_load::docs::{ModuleDocumentation, RecordField};
use roc_load::{ExecutionMode, LoadConfig, LoadedModule, LoadingProblem, Threading};
use roc_module::symbol::{IdentId, Interns, ModuleId, Symbol};
use roc_packaging::cache::{self, RocCacheDir};
use roc_packaging::tarball::build;
use roc_parse::ident::{parse_ident, Accessor, Ident};
use roc_parse::state::State;
use roc_parse::{keyword, type_annotation};
use roc_region::all::Region;
use roc_target::Target;
use roc_types::types::{Alias, Type};
use std::fs;
use std::path::{Path, PathBuf};

pub fn generate_docs_html<'a>(
    arena: &'a Bump,
    pkg_name: &str,
    root_file: impl AsRef<Path>,
    out_dir: impl AsRef<Path>,
    opt_user_specified_base_url: Option<&'a str>,
) -> Result<(), Problem> {
    let loaded_module = load_module_for_docs(root_file.as_ref().to_path_buf());

    // Copy over the assets
    // For debug builds, read assets from fs to speed up build
    // Otherwise, include as string literal

    #[cfg(not(debug_assertions))]
    let assets = {
        let search_js = include_str!("./static/search.js");
        let styles_css = include_str!("./static/styles.css");
        let favicon_svg = include_str!("../../../www/public/favicon.svg");
        let raw_template_html = include_str!("./static/index.html");

        Assets {
            search_js,
            styles_css,
            favicon_svg,
            raw_template_html,
        }
    };

    #[cfg(debug_assertions)]
    let assets = {
        // Construct the absolute path to the static assets
        let workspace_dir = std::env!("ROC_WORKSPACE_DIR");
        let static_dir = Path::new(workspace_dir).join("crates/docs_io/src/static");

        // Read the assets from the filesystem
        let search_js = fs::read_to_string(static_dir.join("search.js")).unwrap();
        let styles_css = fs::read_to_string(static_dir.join("styles.css")).unwrap();
        let favicon_svg =
            fs::read_to_string(static_dir.join("../../../../www/public/favicon.svg")).unwrap();
        let raw_template_html = fs::read_to_string(static_dir.join("index.html")).unwrap();

        Assets {
            search_js,
            styles_css,
            favicon_svg,
            raw_template_html,
        }
    };

    // Clear out generated-docs dir and populate it with static assets
    // (.js, .css, etc.) - note that this does not actually delete the
    // directory if it already existed, but rather deletes all the files
    // inside it. That way, if you have a shell open to that directory,
    // it doesn't get messed up from the dir having been deleted out from
    // under the shell.
    file::populate_build_dir(arena, out_dir.as_ref(), &assets)?;

    IoDocs::new(
        arena,
        &loaded_module,
        assets.raw_template_html.as_ref(),
        // TODO get this from the platform's source file rather than hardcoding it!
        // github.com/roc-lang/roc/issues/5712
        "Documentation",
        opt_user_specified_base_url,
    )
    .generate(out_dir)
}

struct IoDocs<'a> {
    arena: &'a Bump,
    header_doc_comment: &'a str,
    raw_template_html: &'a str,
    pkg_name: &'a str,
    opt_user_specified_base_url: Option<&'a str>,
    sb_entries: Vec<'a, SBEntry<'a>>,
    body_entries_by_module: Vec<'a, (ModuleId, &'a [BodyEntry<'a, Annotation<'a>>])>,
    module_names: Vec<'a, (ModuleId, &'a str)>,
}

impl<'a> IoDocs<'a> {
    pub fn new(
        arena: &'a Bump,
        loaded_module: &LoadedModule,
        raw_template_html: &'a str,
        pkg_name: &'a str,
        opt_user_specified_base_url: Option<&'a str>,
    ) -> IoDocs<'a> {
        let mut module_names =
            Vec::with_capacity_in(loaded_module.exposed_module_docs.len(), arena);
        let mut sb_entries = Vec::with_capacity_in(loaded_module.exposed_modules.len(), arena);
        let mut body_entries_by_module =
            Vec::with_capacity_in(loaded_module.exposed_modules.len(), arena);
        let header_doc_comment = arena.alloc_str(&loaded_module.header_doc_comment);

        for (module_id, docs) in loaded_module.exposed_module_docs.iter() {
            module_names.push((*module_id, &*arena.alloc_str(&docs.name)));

            let mut exposed = Vec::with_capacity_in(docs.exposed_symbols.len(), arena);
            for symbol in docs.exposed_symbols.iter() {
                if let Some(ident_ids) =
                    loaded_module.interns.all_ident_ids.get(&symbol.module_id())
                {
                    if let Some(name) = ident_ids.get_name(symbol.ident_id()) {
                        exposed.push(&*arena.alloc_str(name));
                    }
                }
            }

            sb_entries.push(SBEntry {
                link_text: arena.alloc_str(&docs.name),
                exposed,
                doc_comment: {
                    let todo = (); // TODO: thread through whatever doc comment might be above this.
                                   // Currently, we only know about docs.header_doc_comment, which is
                                   // the doc comment at the top of the module, NOT the doc comment above
                                   // the entry in the package, which is what we want here.
                    ""
                },
            });

            let mut body_entries = Vec::with_capacity_in(docs.entries.len(), arena);

            for entry in docs.entries.iter() {
                if let DocEntry::DocDef(def) = entry {
                    let type_annotation = Annotation {
                        typ: arena.alloc(def.type_annotation.clone()),
                        arena,
                    };
                    body_entries.push(BodyEntry {
                        entry_name: &*arena.alloc_str(&def.name),
                        type_vars_names: Vec::from_iter_in(
                            def.type_vars.iter().map(|s| &*arena.alloc_str(s)),
                            arena,
                        )
                        .into_bump_slice(),
                        type_annotation,
                        docs: def.docs.map(|str| &*arena.alloc_str(&str)),
                    });
                }
            }

            body_entries_by_module.push((*module_id, body_entries.into_bump_slice()));
        }

        Self {
            header_doc_comment,
            sb_entries,
            body_entries_by_module,
            raw_template_html,
            pkg_name,
            opt_user_specified_base_url,
            module_names,
            arena,
        }
    }

    fn generate(self, build_dir: impl AsRef<Path>) -> Result<(), Problem> {
        let arena = &self.arena;
        let build_dir = build_dir.as_ref();

        self.render_to_disk(
            self.arena,
            // Takes the module name to be used as the directory name
            // (or None if this is the root index.html),
            // as well as the contents of the file.
            |opt_module_name, contents| {
                let dir = match opt_module_name {
                    Some(module_name) => PathBuf::from(build_dir)
                        // Replace dots in the module name with slashes
                        .join(module_name.replace('.', std::path::MAIN_SEPARATOR_STR)),
                    None => PathBuf::from(build_dir),
                };

                file::create_dir_all(arena, &dir)?;

                let path_buf = dir.join("index.html");

                file::write(arena, path_buf, contents)
            },
        )
    }

    fn header_doc_comment(&self) -> &'a str {
        self.header_doc_comment
    }
}

#[derive(Debug)]
struct Annotation<'a> {
    arena: &'a Bump,
    typ: &'a TypeAnnotation,
}

impl<'a>
    TypeAnn<
        'a,
        AbilityAnn<'a>,
        slice::Iter<'a, &'a str>,
        slice::Iter<'a, &'a str>,
        slice::Iter<'a, AbilityAnn<'a>>,
        slice::Iter<'a, AbilityMember<'a, Self>>,
    > for Annotation<'a>
{
    fn visit<
        VisitAbility: Fn(slice::Iter<'a, AbilityMember<'a, Self>>, &'a mut String<'a>) + Copy,
        VisitAlias: Fn(slice::Iter<'a, &'a str>, &'a Self, &'a mut String<'a>) + Copy,
        VisitOpaque: Fn(slice::Iter<'a, &'a str>, slice::Iter<'a, AbilityAnn<'a>>, &'a mut String<'a>) + Copy,
        VisitValue: Fn(&'a Self, &'a mut String<'a>) + Copy,
    >(
        &'a self,
        buf: &'a mut String<'a>,
        visit_ability: VisitAbility,
        visit_type_alias: VisitAlias,
        visit_opaque_type: VisitOpaque,
        visit_value: VisitValue,
    ) {
        match &self.typ {
            TypeAnnotation::TagUnion { tags, extension } => {
                if tags.is_empty() {
                    (visit_value)(&self, buf)
                } else {
                    // TODO
                }
            }
            TypeAnnotation::Function {
                args,
                arrow,
                output,
            } => todo!(),
            TypeAnnotation::ObscuredTagUnion => todo!(),
            TypeAnnotation::ObscuredRecord => todo!(),
            TypeAnnotation::BoundVariable(_) => todo!(),
            TypeAnnotation::Apply { name, parts } => {
                buf.push_str(name);

                let mut var_names = Vec::with_capacity_in(parts.len(), self.arena);

                for part in parts {
                    let var_name =
                        self.arena
                            .alloc(bumpalo::collections::String::with_capacity_in(
                                4, self.arena,
                            ));

                    self.visit(
                        var_name,
                        visit_ability,
                        visit_type_alias,
                        visit_opaque_type,
                        visit_value,
                    );

                    var_names.push(var_name.as_str());
                }

                let ability_ann = {
                    let todo = (); // TODO actually get some abilities in there...or do we need them though?

                    &[]
                };

                (visit_opaque_type)(var_names.into_bump_slice().iter(), ability_ann.iter(), buf)
            }
            TypeAnnotation::Record { fields, extension } => {
                if fields.is_empty() {
                    (visit_value)(&self, buf)
                } else {
                    // TODO
                }
            }
            TypeAnnotation::Tuple { elems, extension } => {
                if elems.is_empty() {
                    (visit_value)(&self, buf)
                } else {
                    // TODO
                }
            }
            TypeAnnotation::Ability { members } => todo!(),
            TypeAnnotation::Wildcard => todo!(),
            TypeAnnotation::NoTypeAnn => todo!(),
            TypeAnnotation::Where { ann, implements } => todo!(),
            TypeAnnotation::As { ann, name, vars } => todo!(),
        }
    }
}

#[derive(Debug)]
struct SBEntry<'a> {
    /// In the source code, this will appear in a module's `exposes` list like:
    ///
    /// [
    ///     Foo,
    ///     Bar,
    ///     ## Heading
    ///     Baz,
    ///     Blah,
    /// ]
    pub link_text: &'a str,

    /// The entries this module exposes (types, values, abilities)
    pub exposed: Vec<'a, &'a str>,

    /// These doc comments get interpreted as flat strings; Markdown is not allowed
    /// in them, because they will be rendered in the sidebar as plain text.
    pub doc_comment: &'a str,
}

impl<'a> SidebarEntry<'a, slice::Iter<'a, &'a str>> for SBEntry<'a> {
    fn link_text(&'a self) -> &'a str {
        self.link_text
    }

    fn exposed(&'a self) -> slice::Iter<'a, &'a str> {
        self.exposed.iter()
    }

    fn doc_comment(&'a self) -> &'a str {
        self.doc_comment
    }
}

impl<'a>
    Docs<
        'a,
        AbilityAnn<'a>,
        ModuleId,
        IdentId,
        Annotation<'a>,
        Alias,
        TypeRenderer,
        // Iterators
        slice::Iter<'a, AbilityAnn<'a>>,
        slice::Iter<'a, AbilityMember<'a, Annotation<'a>>>,
        slice::Iter<'a, (ModuleId, &'a str)>,
        SBEntry<'a>,
        slice::Iter<'a, SBEntry<'a>>,
        slice::Iter<'a, &'a str>,
        slice::Iter<'a, BodyEntry<'a, Annotation<'a>>>,
        slice::Iter<'a, (&'a str, slice::Iter<'a, Annotation<'a>>)>,
        slice::Iter<'a, Annotation<'a>>,
    > for IoDocs<'a>
{
    fn package_name(&self) -> &'a str {
        self.pkg_name
    }

    fn raw_template_html(&self) -> &'a str {
        self.raw_template_html
    }

    fn user_specified_base_url(&self) -> Option<&'a str> {
        self.opt_user_specified_base_url
    }

    fn package_doc_comment_markdown(&self) -> &'a str {
        self.header_doc_comment()
    }

    fn base_url(&'a self, module_id: ModuleId) -> &'a str {
        self.user_specified_base_url().unwrap_or("")
    }

    fn module_name(&'a self, module_id: ModuleId) -> &'a str {
        self.module_names()
            .find(|(id, _name)| *id == module_id)
            .map(|(_module_id, name)| *name)
            .unwrap_or_default()
    }

    fn ident_name(&self, module_id: ModuleId, ident_id: IdentId) -> &'a str {
        "TODO ident_name"
    }

    fn opt_alias(&self, module_id: ModuleId, ident_id: IdentId) -> Option<Alias> {
        let todo = ();

        None
    }

    fn module_names(&'a self) -> slice::Iter<'a, (ModuleId, &'a str)> {
        self.module_names.iter()
    }

    fn package_sidebar_entries(&'a self) -> slice::Iter<'a, SBEntry<'a>> {
        self.sb_entries.iter()
    }

    fn body_entries(
        &'a self,
        module_id: ModuleId,
    ) -> slice::Iter<'a, BodyEntry<'a, Annotation<'a>>> {
        self.body_entries_by_module
            .iter()
            .find_map(|(id, entries)| {
                if *id == module_id {
                    Some(*entries)
                } else {
                    None
                }
            })
            .unwrap_or(&[])
            .iter()
    }

    fn opt_type(&'a self, module_id: ModuleId, ident_id: IdentId) -> Option<Annotation<'a>> {
        let todo = ();

        None
    }

    fn visit_type<'b>(
        &self,
        arena: &'b Bump,
        renderer: &mut TypeRenderer,
        typ: &Annotation,
        buf: &mut String<'b>,
    ) {
        let todo = ();
    }
}

#[derive(Debug)]
pub struct AbilityAnn<'a> {
    name: &'a str,
}

impl<'a> AbilityImpl<'a> for AbilityAnn<'a> {
    fn name(&self) -> &'a str {
        todo!()
    }

    fn docs_url(&self) -> &'a str {
        todo!()
    }
}

// fn render_package_index(root_module: &LoadedModule) -> String {
//     // The list items containing module links
//     let mut module_list_buf = String::new();

//     for (_, module) in root_module.docs_by_module.iter() {
//         // The anchor tag containing the module link
//         let mut link_buf = String::new();

//         push_html(
//             &mut link_buf,
//             "a",
//             vec![("href", module.name.as_str())],
//             module.name.as_str(),
//         );

//         push_html(&mut module_list_buf, "li", vec![], link_buf.as_str());
//     }

//     // The HTML for the index page
//     let mut index_buf = String::new();

//     push_html(
//         &mut index_buf,
//         "h2",
//         vec![("class", "module-name")],
//         "Exposed Modules",
//     );
//     push_html(
//         &mut index_buf,
//         "ul",
//         vec![("class", "index-module-links")],
//         module_list_buf.as_str(),
//     );

//     index_buf
// }

// fn render_module_documentation(
//     module: &ModuleDocumentation,
//     root_module: &LoadedModule,
//     all_exposed_symbols: &VecSet<Symbol>,
// ) -> String {
//     let mut buf = String::new();
//     let module_name = module.name.as_str();

//     push_html(&mut buf, "h2", vec![("class", "module-name")], {
//         let mut link_buf = String::new();

//         push_html(&mut link_buf, "a", vec![("href", "/")], module_name);

//         link_buf
//     });

//     for entry in &module.entries {
//         match entry {
//             DocEntry::DocDef(doc_def) => {
//                 // Only render entries that are exposed
//                 if all_exposed_symbols.contains(&doc_def.symbol) {
//                     buf.push_str("<section>");

//                     let def_name = doc_def.name.as_str();
//                     let href = format!("{module_name}#{def_name}");
//                     let mut content = String::new();

//                     push_html(&mut content, "a", vec![("href", href.as_str())], LINK_SVG);
//                     push_html(&mut content, "strong", vec![], def_name);

//                     for type_var in &doc_def.type_vars {
//                         content.push(' ');
//                         content.push_str(type_var.as_str());
//                     }

//                     let type_ann = &doc_def.type_annotation;

//                     if !matches!(type_ann, TypeAnnotation::NoTypeAnn) {
//                         // Ability declarations don't have ":" after the name, just `implements`
//                         if !matches!(type_ann, TypeAnnotation::Ability { .. }) {
//                             content.push_str(" :");
//                         }

//                         content.push(' ');

//                         type_annotation_to_html(0, &mut content, type_ann, false);
//                     }

//                     push_html(
//                         &mut buf,
//                         "h3",
//                         vec![("id", def_name), ("class", "entry-name")],
//                         content.as_str(),
//                     );

//                     if let Some(docs) = &doc_def.docs {
//                         markdown_to_html(
//                             &mut buf,
//                             all_exposed_symbols,
//                             &module.scope,
//                             docs,
//                             root_module,
//                         );
//                     }

//                     buf.push_str("</section>");
//                 }
//             }
//             DocEntry::DetachedDoc(docs) => {
//                 markdown_to_html(
//                     &mut buf,
//                     all_exposed_symbols,
//                     &module.scope,
//                     docs,
//                     root_module,
//                 );
//             }
//         };
//     }

//     buf
// }

// fn push_html(buf: &mut String, tag_name: &str, attrs: Vec<(&str, &str)>, content: impl AsRef<str>) {
//     buf.push('<');
//     buf.push_str(tag_name);

//     for (key, value) in &attrs {
//         buf.push(' ');
//         buf.push_str(key);
//         buf.push_str("=\"");
//         buf.push_str(value);
//         buf.push('"');
//     }

//     if !&attrs.is_empty() {
//         buf.push(' ');
//     }

//     buf.push('>');

//     buf.push_str(content.as_ref());

//     buf.push_str("</");
//     buf.push_str(tag_name);
//     buf.push('>');
// }

// fn base_url() -> String {
//     // e.g. "builtins/" in "https://roc-lang.org/builtins/Str"
//     //
//     // TODO make this a CLI flag to the `docs` subcommand instead of an env var
//     match std::env::var("ROC_DOCS_URL_ROOT") {
//         Ok(root_builtins_path) => {
//             let mut url_str = String::with_capacity(root_builtins_path.len() + 64);

//             if !root_builtins_path.starts_with('/') {
//                 url_str.push('/');
//             }

//             url_str.push_str(&root_builtins_path);

//             if !root_builtins_path.ends_with('/') {
//                 url_str.push('/');
//             }

//             url_str
//         }
//         _ => {
//             let mut url_str = String::with_capacity(64);

//             url_str.push('/');

//             url_str
//         }
//     }
// }

// // TODO render version as well
// fn render_name_link(name: &str) -> String {
//     let mut buf = String::new();

//     push_html(&mut buf, "h1", vec![("class", "pkg-full-name")], {
//         let mut link_buf = String::new();

//         // link to root (= docs overview page)
//         push_html(
//             &mut link_buf,
//             "a",
//             vec![("href", base_url().as_str())],
//             name,
//         );

//         link_buf
//     });

//     buf
// }

// fn render_sidebar<'a, I: Iterator<Item = &'a ModuleDocumentation>>(modules: I) -> String {
//     let mut buf = String::new();

//     for module in modules {
//         let href = module.name.as_str();
//         let mut sidebar_entry_content = String::new();

//         push_html(
//             &mut sidebar_entry_content,
//             "a",
//             vec![("class", "sidebar-module-link"), ("href", href)],
//             module.name.as_str(),
//         );

//         let entries = {
//             let mut entries_buf = String::new();

//             for entry in &module.entries {
//                 if let DocEntry::DocDef(doc_def) = entry {
//                     if module.exposed_symbols.contains(&doc_def.symbol) {
//                         let mut entry_href = String::new();

//                         entry_href.push_str(href);
//                         entry_href.push('#');
//                         entry_href.push_str(doc_def.name.as_str());

//                         push_html(
//                             &mut entries_buf,
//                             "a",
//                             vec![("href", entry_href.as_str())],
//                             doc_def.name.as_str(),
//                         );
//                     }
//                 }
//             }

//             entries_buf
//         };

//         push_html(
//             &mut sidebar_entry_content,
//             "div",
//             vec![("class", "sidebar-sub-entries")],
//             entries.as_str(),
//         );

//         push_html(
//             &mut buf,
//             "div",
//             vec![("class", "sidebar-entry")],
//             sidebar_entry_content.as_str(),
//         );
//     }

//     buf
// }

pub fn load_module_for_docs(filename: PathBuf) -> LoadedModule {
    let arena = Bump::new();
    let load_config = LoadConfig {
        target: Target::LinuxX64, // This is just type-checking for docs, so "target" doesn't matter
        function_kind: roc_solve::FunctionKind::LambdaSet,
        render: roc_reporting::report::RenderTarget::ColorTerminal,
        palette: roc_reporting::report::DEFAULT_PALETTE,
        threading: Threading::AllAvailable,
        exec_mode: ExecutionMode::Check,
    };
    match roc_load::load_and_typecheck(
        &arena,
        filename.clone(),
        Some(filename),
        RocCacheDir::Persistent(cache::roc_cache_dir().as_path()),
        load_config,
    ) {
        Ok(loaded) => loaded,
        Err(LoadingProblem::FormattedReport(report)) => {
            eprintln!("{report}");
            std::process::exit(1);
        }
        Err(e) => panic!("{e:?}"),
    }
}

// const INDENT: &str = "    ";

// fn indent(buf: &mut String, times: usize) {
//     for _ in 0..times {
//         buf.push_str(INDENT);
//     }
// }

// fn new_line(buf: &mut String) {
//     buf.push('\n');
// }

// // html is written to buf
// fn type_annotation_to_html(
//     indent_level: usize,
//     buf: &mut String,
//     type_ann: &TypeAnnotation,
//     needs_parens: bool,
// ) {
//     let is_multiline = should_be_multiline(type_ann);
//     match type_ann {
//         TypeAnnotation::TagUnion { tags, extension } => {
//             if tags.is_empty() {
//                 buf.push_str("[]");
//             } else {
//                 let tags_len = tags.len();

//                 let tag_union_indent = indent_level + 1;

//                 if is_multiline {
//                     new_line(buf);

//                     indent(buf, tag_union_indent);
//                 }

//                 buf.push('[');

//                 if is_multiline {
//                     new_line(buf);
//                 }

//                 let next_indent_level = tag_union_indent + 1;

//                 for (index, tag) in tags.iter().enumerate() {
//                     if is_multiline {
//                         indent(buf, next_indent_level);
//                     }

//                     buf.push_str(tag.name.as_str());

//                     for type_value in &tag.values {
//                         buf.push(' ');
//                         type_annotation_to_html(next_indent_level, buf, type_value, true);
//                     }

//                     if is_multiline {
//                         if index < (tags_len - 1) {
//                             buf.push(',');
//                         }

//                         new_line(buf);
//                     }
//                 }

//                 if is_multiline {
//                     indent(buf, tag_union_indent);
//                 }

//                 buf.push(']');
//             }

//             type_annotation_to_html(indent_level, buf, extension, true);
//         }
//         TypeAnnotation::BoundVariable(var_name) => {
//             buf.push_str(var_name);
//         }
//         TypeAnnotation::Apply { name, parts } => {
//             if parts.is_empty() {
//                 buf.push_str(name);
//             } else {
//                 if needs_parens {
//                     buf.push('(');
//                 }

//                 buf.push_str(name);
//                 for part in parts {
//                     buf.push(' ');
//                     type_annotation_to_html(indent_level, buf, part, true);
//                 }

//                 if needs_parens {
//                     buf.push(')');
//                 }
//             }
//         }
//         TypeAnnotation::Record { fields, extension } => {
//             if fields.is_empty() {
//                 buf.push_str("{}");
//             } else {
//                 let fields_len = fields.len();
//                 let record_indent = indent_level + 1;

//                 if is_multiline {
//                     new_line(buf);
//                     indent(buf, record_indent);
//                 }

//                 buf.push('{');

//                 if is_multiline {
//                     new_line(buf);
//                 }

//                 let next_indent_level = record_indent + 1;

//                 for (index, field) in fields.iter().enumerate() {
//                     if is_multiline {
//                         indent(buf, next_indent_level);
//                     } else {
//                         buf.push(' ');
//                     }

//                     let fields_name = match field {
//                         RecordField::RecordField { name, .. } => name,
//                         RecordField::OptionalField { name, .. } => name,
//                         RecordField::LabelOnly { name } => name,
//                     };

//                     buf.push_str(fields_name.as_str());

//                     match field {
//                         RecordField::RecordField {
//                             type_annotation, ..
//                         } => {
//                             buf.push_str(" : ");
//                             type_annotation_to_html(next_indent_level, buf, type_annotation, false);
//                         }
//                         RecordField::OptionalField {
//                             type_annotation, ..
//                         } => {
//                             buf.push_str(" ? ");
//                             type_annotation_to_html(next_indent_level, buf, type_annotation, false);
//                         }
//                         RecordField::LabelOnly { .. } => {}
//                     }

//                     if is_multiline {
//                         if index < (fields_len - 1) {
//                             buf.push(',');
//                         }

//                         new_line(buf);
//                     }
//                 }

//                 if is_multiline {
//                     indent(buf, record_indent);
//                 } else {
//                     buf.push(' ');
//                 }

//                 buf.push('}');
//             }

//             type_annotation_to_html(indent_level, buf, extension, true);
//         }
//         TypeAnnotation::Function { args, output } => {
//             let mut paren_is_open = false;
//             let mut peekable_args = args.iter().peekable();

//             while let Some(arg) = peekable_args.next() {
//                 if is_multiline {
//                     if !should_be_multiline(arg) {
//                         new_line(buf);
//                     }
//                     indent(buf, indent_level + 1);
//                 }
//                 if needs_parens && !paren_is_open {
//                     buf.push('(');
//                     paren_is_open = true;
//                 }

//                 let child_needs_parens = matches!(arg, TypeAnnotation::Function { .. });
//                 type_annotation_to_html(indent_level, buf, arg, child_needs_parens);

//                 if peekable_args.peek().is_some() {
//                     buf.push_str(", ");
//                 }
//             }

//             if is_multiline {
//                 new_line(buf);
//                 indent(buf, indent_level + 1);
//             } else {
//                 buf.push(' ');
//             }

//             buf.push_str("-> ");

//             let mut next_indent_level = indent_level;

//             if should_be_multiline(output) {
//                 next_indent_level += 1;
//             }

//             type_annotation_to_html(next_indent_level, buf, output, false);
//             if needs_parens && paren_is_open {
//                 buf.push(')');
//             }
//         }
//         TypeAnnotation::Ability { members } => {
//             buf.push_str(keyword::IMPLEMENTS);

//             for member in members {
//                 new_line(buf);
//                 indent(buf, indent_level + 1);

//                 // TODO use member.docs somehow. This doesn't look good though:
//                 // if let Some(docs) = &member.docs {
//                 //     buf.push_str("## ");
//                 //     buf.push_str(docs);

//                 //     new_line(buf);
//                 //     indent(buf, indent_level + 1);
//                 // }

//                 buf.push_str(&member.name);
//                 buf.push_str(" : ");

//                 type_annotation_to_html(indent_level + 1, buf, &member.type_annotation, false);

//                 if !member.able_variables.is_empty() {
//                     new_line(buf);
//                     indent(buf, indent_level + 2);
//                     buf.push_str(keyword::WHERE);

//                     for (index, (name, type_anns)) in member.able_variables.iter().enumerate() {
//                         if index != 0 {
//                             buf.push(',');
//                         }

//                         buf.push(' ');
//                         buf.push_str(name);
//                         buf.push(' ');
//                         buf.push_str(keyword::IMPLEMENTS);

//                         for (index, ann) in type_anns.iter().enumerate() {
//                             if index != 0 {
//                                 buf.push_str(" &");
//                             }

//                             buf.push(' ');

//                             type_annotation_to_html(indent_level + 2, buf, ann, false);
//                         }
//                     }
//                 }
//             }
//         }
//         TypeAnnotation::ObscuredTagUnion => {
//             buf.push_str("[@..]");
//         }
//         TypeAnnotation::ObscuredRecord => {
//             buf.push_str("{ @.. }");
//         }
//         TypeAnnotation::NoTypeAnn => {}
//         TypeAnnotation::Wildcard => buf.push('*'),
//         TypeAnnotation::Tuple { elems, extension } => {
//             let elems_len = elems.len();
//             let tuple_indent = indent_level + 1;

//             if is_multiline {
//                 new_line(buf);
//                 indent(buf, tuple_indent);
//             }

//             buf.push('(');

//             if is_multiline {
//                 new_line(buf);
//             }

//             let next_indent_level = tuple_indent + 1;

//             for (index, elem) in elems.iter().enumerate() {
//                 if is_multiline {
//                     indent(buf, next_indent_level);
//                 }

//                 type_annotation_to_html(next_indent_level, buf, elem, false);

//                 if is_multiline {
//                     if index < (elems_len - 1) {
//                         buf.push(',');
//                     }

//                     new_line(buf);
//                 }
//             }

//             if is_multiline {
//                 indent(buf, tuple_indent);
//             }

//             buf.push(')');

//             type_annotation_to_html(indent_level, buf, extension, true);
//         }
//         TypeAnnotation::Where { ann, implements } => {
//             type_annotation_to_html(indent_level, buf, ann, false);

//             new_line(buf);
//             indent(buf, indent_level + 1);

//             buf.push_str(keyword::WHERE);

//             let multiline_implements = implements
//                 .iter()
//                 .any(|imp| imp.abilities.iter().any(should_be_multiline));

//             for (index, imp) in implements.iter().enumerate() {
//                 if index != 0 {
//                     buf.push(',');
//                 }

//                 if multiline_implements {
//                     new_line(buf);
//                     indent(buf, indent_level + 2);
//                 } else {
//                     buf.push(' ')
//                 }

//                 buf.push_str(&imp.name);
//                 buf.push(' ');
//                 buf.push_str(keyword::IMPLEMENTS);
//                 buf.push(' ');

//                 for (index, ability) in imp.abilities.iter().enumerate() {
//                     if index != 0 {
//                         buf.push_str(" & ");
//                     }

//                     type_annotation_to_html(indent_level, buf, ability, false);
//                 }
//             }
//         }
//         TypeAnnotation::As { ann, name, vars } => {
//             type_annotation_to_html(indent_level, buf, ann, true);
//             buf.push(' ');
//             buf.push_str(name);

//             for var in vars {
//                 buf.push(' ');
//                 buf.push_str(var);
//             }
//         }
//     }
// }

// fn should_be_multiline(type_ann: &TypeAnnotation) -> bool {
//     match type_ann {
//         TypeAnnotation::TagUnion { tags, extension } => {
//             tags.len() > 1
//                 || should_be_multiline(extension)
//                 || tags
//                     .iter()
//                     .any(|tag| tag.values.iter().any(should_be_multiline))
//         }
//         TypeAnnotation::Function { args, output } => {
//             args.len() > 2 || should_be_multiline(output) || args.iter().any(should_be_multiline)
//         }
//         TypeAnnotation::ObscuredTagUnion => false,
//         TypeAnnotation::ObscuredRecord => false,
//         TypeAnnotation::BoundVariable(_) => false,
//         TypeAnnotation::Apply { parts, .. } => parts.iter().any(should_be_multiline),
//         TypeAnnotation::Record { fields, extension } => {
//             fields.len() > 1
//                 || should_be_multiline(extension)
//                 || fields.iter().any(|field| match field {
//                     RecordField::RecordField {
//                         type_annotation, ..
//                     } => should_be_multiline(type_annotation),
//                     RecordField::OptionalField {
//                         type_annotation, ..
//                     } => should_be_multiline(type_annotation),
//                     RecordField::LabelOnly { .. } => false,
//                 })
//         }
//         TypeAnnotation::Ability { .. } => true,
//         TypeAnnotation::Wildcard => false,
//         TypeAnnotation::NoTypeAnn => false,
//         TypeAnnotation::Tuple { elems, extension } => {
//             elems.len() > 1
//                 || should_be_multiline(extension)
//                 || elems.iter().any(should_be_multiline)
//         }
//         TypeAnnotation::Where { ann, implements } => {
//             should_be_multiline(ann)
//                 || implements
//                     .iter()
//                     .any(|imp| imp.abilities.iter().any(should_be_multiline))
//         }
//         TypeAnnotation::As {
//             ann,
//             name: _,
//             vars: _,
//         } => should_be_multiline(ann),
//     }
// }

// fn name_from_ident_id(&self, ident_id: IdentId, ident_ids: &'a IdentIds) -> &'a str {
//     ident_ids.get_name(ident_id).unwrap_or_else(|| {
//             if cfg!(debug_assertions) {
//                 unreachable!("docs generation tried to render a relative URL for IdentId {:?} but it was not found in home_identids, which should never happen!", ident_id);
//             }

//             // In release builds, don't panic, just gracefully continue
//             // by not writing the url. It'll be a bug, but at least
//             // it won't block the user from seeing *some* docs rendered.
//             ""
//         })
// }
