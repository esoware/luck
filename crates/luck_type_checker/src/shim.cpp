#include "Luau/Ast.h"
#include "Luau/AstQuery.h"
#include "Luau/Autocomplete.h"
#include "Luau/AutocompleteTypes.h"
#include "Luau/BuiltinDefinitions.h"
#include "Luau/Cancellation.h"
#include "Luau/Common.h"
#include "Luau/Config.h"
#include "Luau/ConfigResolver.h"
#include "Luau/ConstraintSolver.h"
#include "Luau/Error.h"
#include "Luau/ExperimentalFlags.h"
#include "Luau/FileResolver.h"
#include "Luau/Frontend.h"
#include "Luau/LuauConfig.h"
#include "Luau/Module.h"
#include "Luau/Scope.h"
#include "Luau/TimeTrace.h"
#include "Luau/ToString.h"
#include "Luau/Type.h"
#include "Luau/TypeArena.h"
#include "Luau/TypeCheckLimits.h"
#include "Luau/TypeFwd.h"
#include "Luau/TypePack.h"
#include "Luau/Variant.h"
#include "lua.h"

#include <cstdint>
#include <cstring>
#include <functional>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <utility>
#include <vector>

using Task = std::function<void()>;
using CancellationToken = std::shared_ptr<Luau::FrontendCancellationToken>;

extern "C" {

struct LuckStr {
    const char* data;
    size_t len;
};

struct LuckLocation {
    uint32_t begin_line;
    uint32_t begin_column;
    uint32_t end_line;
    uint32_t end_column;
};

struct LuckHost {
    void* context;
    bool (*source)(void* context, LuckStr module, std::string* source);
    bool (*resolve)(void* context, LuckStr requirer, LuckStr path, std::string* module);
    void (*config)(
        void* context,
        LuckStr module,
        Luau::Config* config,
        const CancellationToken* cancellation
    );
};

struct LuckError {
    LuckStr module;
    bool is_syntax_error;
    bool has_location;
    LuckLocation location;
    LuckStr message;
};

struct LuckErrors {
    void* context;
    void (*push)(void* context, const LuckError* error);
};

struct LuckModule {
    LuckStr name;
    LuckErrors errors;
};

struct LuckExecutor {
    void* context;
    void (*submit)(void* context, Task* task);
};

struct LuckDocs {
    LuckStr symbol;
    LuckStr module;
    LuckLocation location;
};

struct LuckRange {
    uint32_t start;
    uint32_t end;
};

struct LuckDescribed {
    void* context;
    void (*describe)(void* context, LuckLocation location, LuckStr ty, const LuckDocs* docs);
    void (*signature)(void* context, LuckStr label, const LuckRange* params, size_t count);
};

struct LuckEntry {
    LuckStr name;
    uint8_t kind;
    LuckStr ty;
    bool is_deprecated;
    LuckStr insert_text;
    LuckDocs docs;
};

struct LuckEntries {
    void* context;
    void (*push)(void* context, const LuckEntry* entry);
};

struct LuckAliases {
    void* context;
    void (*push)(void* context, LuckStr name, LuckStr value);
};
}

namespace {

constexpr double CONFIG_TIME_LIMIT_SECONDS = 1.0;

LuckStr borrowed(const std::string& text) {
    return {text.data(), text.size()};
}

LuckStr borrowed(const std::optional<std::string>& text) {
    return text ? borrowed(*text) : LuckStr{};
}

std::string owned(LuckStr text) {
    return {text.data, text.len};
}

LuckLocation location(const Luau::Location& at) {
    return {at.begin.line, at.begin.column, at.end.line, at.end.column};
}

void push(
    LuckErrors errors,
    const std::string& module,
    bool is_syntax_error,
    const std::optional<Luau::Location>& at,
    const std::string& message
) {
    LuckError error{
        borrowed(module),
        is_syntax_error,
        at.has_value(),
        at ? location(*at) : LuckLocation{},
        borrowed(message),
    };
    errors.push(errors.context, &error);
}

void set_flags() {
    static std::once_flag FLAGS_SET;
    std::call_once(FLAGS_SET, [] {
        for (Luau::FValue<bool>* flag = Luau::FValue<bool>::list; flag; flag = flag->next) {
            if (std::strncmp(flag->name, "Luau", 4) == 0
                && !Luau::isAnalysisFlagExperimental(flag->name)) {
                flag->value = true;
            }
        }
    });
}

Luau::ToStringOptions type_options() {
    Luau::ToStringOptions options;
    options.functionTypeArguments = true;
    return options;
}

struct Unfrozen {
    explicit Unfrozen(Luau::TypeArena& arena) : arena(arena) {
        Luau::unfreeze(arena);
    }

    ~Unfrozen() {
        Luau::freeze(arena);
    }

    Luau::TypeArena& arena;
};

struct Checker final : Luau::FileResolver, Luau::ConfigResolver {
    explicit Checker(LuckHost host) : host(host), frontend(Luau::SolverMode::New, this, this) {
        Luau::registerBuiltinGlobals(frontend, frontend.globals);
        Luau::freeze(frontend.globals.globalTypes);
    }

    std::optional<Luau::SourceCode> readSource(const Luau::ModuleName& name) override {
        Luau::SourceCode code{{}, Luau::SourceCode::Module};
        if (!host.source(host.context, borrowed(name), &code.source)) {
            return std::nullopt;
        }
        return code;
    }

    std::optional<Luau::ModuleInfo> resolveModule(
        const Luau::ModuleInfo* context,
        Luau::AstExpr* expr,
        const Luau::TypeCheckLimits&
    ) override {
        const auto* path = expr->as<Luau::AstExprConstantString>();
        Luau::ModuleInfo info;
        if (!context || !path
            || !host.resolve(
                host.context,
                borrowed(context->name),
                {path->value.data, path->value.size},
                &info.name
            )) {
            return std::nullopt;
        }
        return info;
    }

    const Luau::Config& getConfig(
        const Luau::ModuleName& name,
        const Luau::TypeCheckLimits& limits
    ) const override {
        config = Luau::Config{};
        host.config(host.context, borrowed(name), &config, &limits.cancellationToken);
        return config;
    }

    LuckHost host;
    mutable Luau::Config config;
    Luau::Frontend frontend;
};

void push_type_error(Checker& checker, LuckErrors errors, const Luau::TypeError& error) {
    bool is_syntax_error = Luau::get_if<Luau::SyntaxError>(&error.data) != nullptr;
    std::string message = Luau::toString(error, {&checker});
    push(errors, error.moduleName, is_syntax_error, error.location, message);
}

struct NamedTypeMagic final : Luau::MagicFunction {
    NamedTypeMagic(const Luau::Scope* scope, const Luau::ExternType* returned)
        : scope(scope), returned(returned) {}

    std::optional<Luau::WithPredicate<Luau::TypePackId>> handleOldSolver(
        Luau::TypeChecker&,
        const Luau::ScopePtr&,
        const Luau::AstExprCall&,
        Luau::WithPredicate<Luau::TypePackId>
    ) override {
        return std::nullopt;
    }

    bool infer(const Luau::MagicFunctionCallContext& context) override {
        const Luau::AstArray<Luau::AstExpr*>& args = context.callSite->args;
        const auto* literal =
            args.size > 0 ? args.data[0]->as<Luau::AstExprConstantString>() : nullptr;
        if (!literal) {
            return false;
        }
        std::string name(literal->value.data, literal->value.size);
        auto named = scope->exportedTypeBindings.find(name);
        const auto* ty = named == scope->exportedTypeBindings.end()
            ? nullptr
            : Luau::get<Luau::ExternType>(Luau::follow(named->second.type));
        if (!ty || !Luau::isSubclass(ty, returned)) {
            context.solver->reportError(
                Luau::UnknownSymbol{std::move(name), Luau::UnknownSymbol::Type},
                literal->location,
                *context.constraint->moduleName
            );
            return false;
        }
        Luau::TypePackId pack = context.solver->arena->addTypePack({named->second.type});
        Luau::asMutable(context.result)->ty.emplace<Luau::BoundTypePack>(pack);
        return true;
    }

    const Luau::Scope* scope;
    const Luau::ExternType* returned;
};

std::pair<const Luau::Property*, Luau::TypeId> find_property(
    Luau::TypeId owner,
    const std::string& name
) {
    owner = Luau::follow(owner);
    if (const auto* table = Luau::get<Luau::TableType>(owner)) {
        auto property = table->props.find(name);
        if (property == table->props.end()) {
            return {};
        }
        return {&property->second, owner};
    }
    if (const auto* extern_type = Luau::get<Luau::ExternType>(owner)) {
        auto property = extern_type->props.find(name);
        if (property != extern_type->props.end()) {
            return {&property->second, owner};
        }
        if (!extern_type->parent) {
            return {};
        }
        return find_property(*extern_type->parent, name);
    }
    if (const auto* intersection = Luau::get<Luau::IntersectionType>(owner)) {
        for (Luau::TypeId part : intersection->parts) {
            if (auto found = find_property(part, name); found.first) {
                return found;
            }
        }
        return {};
    }
    const auto* primitive = Luau::get<Luau::PrimitiveType>(owner);
    const auto* metatable = primitive && primitive->metatable
        ? Luau::get<Luau::TableType>(Luau::follow(*primitive->metatable))
        : nullptr;
    if (!metatable) {
        return {};
    }
    auto index = metatable->props.find("__index");
    if (index == metatable->props.end() || !index->second.readTy) {
        return {};
    }
    return find_property(*index->second.readTy, name);
}

struct Docs {
    std::optional<std::string> symbol;
    std::optional<std::string> module;
    Luau::Location declaration;

    void add_property(Luau::TypeId owner, const std::string& name) {
        auto [property, declaring] = find_property(owner, name);
        if (!property) {
            return;
        }
        if (!symbol) {
            symbol = property->documentationSymbol;
        }
        std::optional<Luau::Location> at =
            property->location ? property->location : property->typeLocation;
        std::optional<Luau::ModuleName> definitions = Luau::getDefinitionModuleName(declaring);
        if (at && definitions) {
            module = std::move(definitions);
            declaration = *at;
        }
    }

    LuckDocs raw() const {
        return {borrowed(symbol), borrowed(module), location(declaration)};
    }
};

void push_signatures(Luau::TypeId ty, bool is_method_call, LuckDescribed described) {
    ty = Luau::follow(ty);
    std::vector<Luau::TypeId> overloads{ty};
    if (const auto* intersection = Luau::get<Luau::IntersectionType>(ty)) {
        overloads = intersection->parts;
    }
    for (Luau::TypeId overload : overloads) {
        const auto* function = Luau::get<Luau::FunctionType>(Luau::follow(overload));
        if (!function) {
            continue;
        }
        Luau::ToStringOptions options = type_options();
        options.hideFunctionSelfArgument = is_method_call;
        std::string label = Luau::toStringNamedFunction("", *function, options);
        std::vector<LuckRange> params;
        size_t from = label.find('(');
        auto add_param = [&](const std::string& text) {
            from = from == std::string::npos ? from : label.find(text, from);
            if (from != std::string::npos) {
                params.push_back({uint32_t(from), uint32_t(from + text.size())});
                from += text.size();
            }
        };
        Luau::TypePackIterator param = Luau::begin(function->argTypes);
        for (size_t index = 0; param != Luau::end(function->argTypes); ++param, ++index) {
            if (index == 0 && function->hasSelf && is_method_call) {
                continue;
            }
            bool is_named = index < function->argNames.size() && function->argNames[index];
            std::string name = is_named ? function->argNames[index]->name : "_";
            add_param(name + ": " + Luau::toString(*param, options));
        }
        if (std::optional<Luau::TypePackId> tail = param.tail()) {
            const auto* variadic = Luau::get<Luau::VariadicTypePack>(Luau::follow(*tail));
            if (!variadic || !variadic->hidden) {
                add_param(
                    "...: "
                    + (variadic ? Luau::toString(variadic->ty, options)
                                : Luau::toString(*tail, options))
                );
            }
        }
        described.signature(described.context, borrowed(label), params.data(), params.size());
    }
}

}

extern "C" {

void luck_string_assign(std::string* string, LuckStr value) noexcept {
    string->assign(value.data, value.len);
}

void luck_config_apply_luaurc(Luau::Config* config, LuckStr text, LuckErrors errors) noexcept {
    Luau::ConfigOptions options;
    options.aliasOptions = Luau::ConfigOptions::AliasOptions{std::nullopt, true};
    if (std::optional<std::string> error = Luau::parseConfig(owned(text), *config, options)) {
        push(errors, "", false, std::nullopt, *error);
    }
}

void luck_config_apply_luau_config(
    Luau::Config* config,
    const CancellationToken* cancellation,
    LuckStr source,
    LuckErrors errors
) noexcept try {
    Luau::TypeCheckLimits limits;
    limits.finishTime = Luau::TimeTrace::getClock() + CONFIG_TIME_LIMIT_SECONDS;
    limits.cancellationToken = *cancellation;
    Luau::InterruptCallbacks callbacks;
    callbacks.initCallback = [&limits](lua_State* state) {
        lua_callbacks(state)->userdata = &limits;
    };
    callbacks.interruptCallback = [](lua_State* state, int) {
        const auto* limits =
            static_cast<const Luau::TypeCheckLimits*>(lua_callbacks(state)->userdata);
        if (Luau::TimeTrace::getClock() > *limits->finishTime) {
            throw Luau::TimeLimitError(Luau::kLuauConfigName);
        }
        if (limits->cancellationToken && limits->cancellationToken->requested()) {
            throw Luau::UserCancelError(Luau::kLuauConfigName);
        }
    };
    std::optional<std::string> error = Luau::extractLuauConfig(
        owned(source),
        *config,
        Luau::ConfigOptions::AliasOptions{std::nullopt, true},
        std::move(callbacks)
    );
    if (error) {
        push(errors, "", false, std::nullopt, *error);
    }
} catch (const Luau::InternalCompilerError& error) {
    push(errors, "", false, std::nullopt, error.message);
}

void luck_luau_config_aliases(LuckStr source, LuckErrors errors, LuckAliases aliases) noexcept {
    Luau::Config config;
    CancellationToken none;
    luck_config_apply_luau_config(&config, &none, source, errors);
    for (const auto& [name, alias] : config.aliases) {
        aliases.push(aliases.context, borrowed(name), borrowed(alias.value));
    }
}

Luau::Config* luck_config_save(const Luau::Config* config) noexcept {
    return new Luau::Config(*config);
}

void luck_config_set(Luau::Config* config, const Luau::Config* saved) noexcept {
    *config = *saved;
}

void luck_config_free(Luau::Config* config) noexcept {
    delete config;
}

Checker* luck_checker_new(LuckHost host) noexcept {
    set_flags();
    return new Checker(host);
}

void luck_checker_free(Checker* checker) noexcept {
    delete checker;
}

void luck_checker_load_definitions(
    Checker* checker,
    LuckStr name,
    LuckStr source,
    LuckErrors errors
) noexcept try {
    Luau::Frontend& frontend = checker->frontend;
    Unfrozen unfrozen(frontend.globals.globalTypes);
    std::string package = owned(name);
    Luau::LoadDefinitionFileResult result = frontend.loadDefinitionFile(
        frontend.globals,
        frontend.globals.globalScope,
        {source.data, source.len},
        package,
        false
    );
    for (const Luau::ParseError& error : result.parseResult.errors) {
        push(errors, package, true, error.getLocation(), error.getMessage());
    }
    if (result.module) {
        for (const Luau::TypeError& error : result.module->errors) {
            push_type_error(*checker, errors, error);
        }
    }
} catch (const Luau::InternalCompilerError& error) {
    push(errors, owned(name), false, error.location, error.message);
}

void luck_checker_return_named_type(Checker* checker, LuckStr owner, LuckStr member) noexcept try {
    Luau::GlobalTypes& globals = checker->frontend.globals;
    const auto& types = globals.globalScope->exportedTypeBindings;
    std::string owner_name = owned(owner);
    const Luau::TableType::Props* props = nullptr;
    if (std::optional<Luau::Binding> global = Luau::tryGetGlobalBinding(globals, owner_name)) {
        if (const auto* table = Luau::get<Luau::TableType>(Luau::follow(global->typeId))) {
            props = &table->props;
        }
    } else if (auto named = types.find(owner_name); named != types.end()) {
        if (const auto* ty = Luau::get<Luau::ExternType>(Luau::follow(named->second.type))) {
            props = &ty->props;
        }
    }
    if (!props) {
        return;
    }
    auto property = props->find(owned(member));
    if (property == props->end() || !property->second.readTy) {
        return;
    }
    Luau::TypeId function = Luau::follow(*property->second.readTy);
    const auto* signature = Luau::get<Luau::FunctionType>(function);
    std::optional<Luau::TypeId> result =
        signature ? Luau::first(signature->retTypes) : std::nullopt;
    const auto* returned = result ? Luau::get<Luau::ExternType>(Luau::follow(*result)) : nullptr;
    if (!returned) {
        return;
    }
    Unfrozen unfrozen(globals.globalTypes);
    Luau::attachMagicFunction(
        function,
        std::make_shared<NamedTypeMagic>(globals.globalScope.get(), returned)
    );
} catch (const Luau::InternalCompilerError&) {}

void luck_checker_bind_type_namespace(
    Checker* checker,
    LuckStr type_namespace,
    LuckStr supertype
) noexcept try {
    Luau::GlobalTypes& globals = checker->frontend.globals;
    auto& types = globals.globalScope->exportedTypeBindings;
    auto super = types.find(owned(supertype));
    if (super == types.end()) {
        return;
    }
    Luau::TypeId super_ty = Luau::follow(super->second.type);
    std::string prefix = owned(type_namespace);
    auto& bound = globals.globalScope->importedTypeBindings[prefix];
    Unfrozen unfrozen(globals.globalTypes);
    for (auto binding = types.begin(); binding != types.end();) {
        auto* ty = Luau::getMutable<Luau::ExternType>(Luau::follow(binding->second.type));
        const std::string& name = binding->first;
        if (ty && ty->parent && Luau::follow(*ty->parent) == super_ty && name.size() > prefix.size()
            && name.compare(0, prefix.size(), prefix) == 0) {
            std::string rest = name.substr(prefix.size());
            ty->name = prefix + "." + rest;
            bound.emplace(std::move(rest), binding->second);
            binding = types.erase(binding);
        } else {
            ++binding;
        }
    }
} catch (const Luau::InternalCompilerError&) {}

void luck_checker_retain_type_graphs(Checker* checker) noexcept {
    checker->frontend.options.retainFullTypeGraphs = true;
}

void luck_checker_mark_dirty(Checker* checker, LuckStr module) noexcept {
    checker->frontend.markDirty(owned(module));
}

void luck_checker_check(
    Checker* checker,
    const LuckModule* modules,
    size_t count,
    const CancellationToken* cancellation,
    LuckExecutor executor,
    LuckErrors errors
) noexcept try {
    Luau::Frontend& frontend = checker->frontend;
    std::vector<Luau::ModuleName> names;
    for (size_t index = 0; index < count; ++index) {
        names.push_back(owned(modules[index].name));
    }
    Luau::FrontendOptions options = frontend.options;
    options.cancellationToken = *cancellation;
    frontend.queueModuleCheck(names);
    frontend.checkQueuedModules(options, [executor](std::vector<Task> tasks) {
        for (Task& task : tasks) {
            executor.submit(executor.context, new Task(std::move(task)));
        }
    });
    if ((*cancellation)->requested()) {
        return;
    }
    for (size_t index = 0; index < count; ++index) {
        if (std::optional<Luau::CheckResult> result =
                frontend.getCheckResult(names[index], false)) {
            for (const Luau::TypeError& error : result->errors) {
                push_type_error(*checker, modules[index].errors, error);
            }
        }
    }
} catch (const Luau::InternalCompilerError& error) {
    push(errors, error.moduleName.value_or(""), false, error.location, error.message);
}

void luck_checker_describe(
    Checker* checker,
    LuckStr module,
    uint32_t line,
    uint32_t column,
    LuckDescribed described
) noexcept try {
    Luau::Frontend& frontend = checker->frontend;
    std::string name = owned(module);
    const Luau::SourceModule* source = frontend.getSourceModule(name);
    Luau::ModulePtr checked = frontend.moduleResolver.getModule(name);
    if (!source || !checked) {
        return;
    }
    Luau::Position position{line, column};
    std::optional<Luau::TypeId> ty;
    Luau::Location at;
    Docs docs;
    bool is_method_call = false;
    std::vector<Luau::AstNode*> ancestry = Luau::findAstAncestryOfPosition(*source, position, true);
    if (!ancestry.empty() && ancestry.back()->asType()) {
        const auto* reference = ancestry.back()->as<Luau::AstTypeReference>();
        Luau::ScopePtr scope = Luau::findScopeAtPosition(*checked, position);
        if (!reference || !scope) {
            return;
        }
        std::optional<Luau::TypeFun> named = reference->prefix
            ? scope->lookupImportedType(reference->prefix->value, reference->name.value)
            : scope->lookupType(reference->name.value);
        if (!named) {
            return;
        }
        ty = named->type;
        at = reference->location;
        docs.symbol = Luau::follow(named->type)->documentationSymbol;
    } else {
        Luau::ExprOrLocal found = Luau::findExprOrLocalAtPosition(*source, position);
        if (Luau::AstExpr* expr = found.getExpr()) {
            if (const Luau::TypeId* expr_ty = checked->astTypes.find(expr)) {
                ty = *expr_ty;
            }
            const auto* index = expr->as<Luau::AstExprIndexName>();
            if (const Luau::TypeId* owner = index ? checked->astTypes.find(index->expr) : nullptr) {
                docs.add_property(*owner, index->index.value);
            }
            is_method_call = index && index->op == ':';
        } else if (Luau::AstLocal* local = found.getLocal()) {
            if (Luau::ScopePtr scope = Luau::findScopeAtPosition(*checked, position)) {
                ty = scope->lookup(local);
            }
        }
        if (!ty || !found.getLocation()) {
            return;
        }
        at = *found.getLocation();
        if (!docs.symbol) {
            docs.symbol = Luau::getDocumentationSymbolAtPosition(*source, *checked, position);
        }
    }
    std::string text = Luau::toString(*ty, type_options());
    LuckDocs raw = docs.raw();
    described.describe(described.context, location(at), borrowed(text), &raw);
    push_signatures(*ty, is_method_call, described);
} catch (const Luau::InternalCompilerError&) {}

void luck_checker_autocomplete(
    Checker* checker,
    LuckStr module,
    uint32_t line,
    uint32_t column,
    LuckEntries entries
) noexcept try {
    Luau::Frontend& frontend = checker->frontend;
    std::string name = owned(module);
    Luau::ModulePtr checked = frontend.moduleResolver.getModule(name);
    if (!checked) {
        return;
    }
    Luau::AutocompleteResult result =
        Luau::autocomplete(frontend, name, Luau::Position{line, column}, [](auto...) {
            return std::optional<Luau::AutocompleteEntryMap>{};
        });
    const auto* index =
        result.ancestry.empty() ? nullptr : result.ancestry.back()->as<Luau::AstExprIndexName>();
    const Luau::TypeId* owner = index ? checked->astTypes.find(index->expr) : nullptr;
    Luau::ToStringOptions options = type_options();
    for (const auto& [entry_name, entry] : result.entryMap) {
        if (entry.wrongIndexType) {
            continue;
        }
        Docs docs;
        docs.symbol = entry.documentationSymbol;
        if (owner) {
            docs.add_property(*owner, entry_name);
        }
        std::optional<std::string> ty;
        if (entry.type) {
            ty = Luau::toString(*entry.type, options);
        }
        LuckEntry raw{
            borrowed(entry_name),
            uint8_t(entry.kind),
            borrowed(ty),
            entry.deprecated,
            borrowed(entry.insertText),
            docs.raw(),
        };
        entries.push(entries.context, &raw);
    }
} catch (const Luau::InternalCompilerError&) {}

void luck_task_run(Task* task) noexcept {
    std::unique_ptr<Task> owned_task(task);
    (*owned_task)();
}

CancellationToken* luck_cancellation_new() noexcept {
    return new CancellationToken(std::make_shared<Luau::FrontendCancellationToken>());
}

void luck_cancellation_cancel(const CancellationToken* token) noexcept {
    (*token)->cancel();
}

bool luck_cancellation_is_cancelled(const CancellationToken* token) noexcept {
    return *token && (*token)->requested();
}

void luck_cancellation_free(CancellationToken* token) noexcept {
    delete token;
}

LuckStr luck_builtin_definitions() noexcept {
    set_flags();
    static const std::string SOURCE = Luau::getBuiltinDefinitionSource();
    return borrowed(SOURCE);
}
}
