// The C ABI that ffi.rs declares. Every function is noexcept: an exception
// Analysis documents, an InternalCompilerError or one of its kinds, is caught
// and reported, and any other, such as std::bad_alloc, terminates the
// process, as running out of memory does in Rust.

#include "Luau/Ast.h"
#include "Luau/AstQuery.h"
#include "Luau/Autocomplete.h"
#include "Luau/BuiltinDefinitions.h"
#include "Luau/Common.h"
#include "Luau/Config.h"
#include "Luau/ConfigResolver.h"
#include "Luau/ConstraintSolver.h"
#include "Luau/Error.h"
#include "Luau/ExperimentalFlags.h"
#include "Luau/FileResolver.h"
#include "Luau/Frontend.h"
#include "Luau/Scope.h"
#include "Luau/ToString.h"
#include "Luau/Type.h"
#include "Luau/TypeArena.h"
#include "Luau/TypeCheckLimits.h"
#include "Luau/TypePack.h"

#include <cstdint>
#include <cstring>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <unordered_map>
#include <utility>
#include <vector>

LUAU_FASTFLAG(LuauCyclicRequireTypeInference)

extern "C"
{

struct LuckStr
{
    const char* data;
    size_t len;
};

struct LuckHost
{
    void* context;
    bool (*source)(void* context, LuckStr module, void* source);
    bool (*resolve)(void* context, LuckStr requirer, LuckStr path, void* module);
    void (*config)(void* context, LuckStr module, void* config);
};

struct LuckError
{
    size_t module;
    bool is_syntax_error;
    bool has_location;
    uint32_t begin_line;
    uint32_t begin_column;
    uint32_t end_line;
    uint32_t end_column;
    LuckStr module_name;
    LuckStr message;
};

struct LuckErrors
{
    void* context;
    void (*push)(void* context, const LuckError* error);
};

struct LuckExecutor
{
    void* context;
    void (*submit)(void* context, void* task);
};

struct LuckLocation
{
    uint32_t begin_line;
    uint32_t begin_column;
    uint32_t end_line;
    uint32_t end_column;
};

struct LuckDocs
{
    bool has_symbol;
    LuckStr symbol;
    bool has_definition;
    LuckStr module;
    LuckLocation location;
};

struct LuckDescribed
{
    void* context;
    void (*describe)(void* context, LuckLocation location, LuckStr type, const LuckDocs* docs);
    void (*signature)(void* context, LuckStr label, const uint32_t* params, size_t count);
};

struct LuckEntry
{
    LuckStr name;
    uint8_t kind;
    bool has_type;
    LuckStr type;
    bool is_deprecated;
    bool has_insert_text;
    LuckStr insert_text;
    LuckDocs docs;
};

struct LuckEntries
{
    void* context;
    void (*push)(void* context, const LuckEntry* entry);
};

enum : uint8_t
{
    LUCK_CHECKED,
    LUCK_CANCELLED,
    LUCK_INTERNAL_ERROR,
};

} // extern "C"

namespace
{

using CancellationToken = std::shared_ptr<Luau::FrontendCancellationToken>;

LuckStr str(const std::string& value)
{
    return {value.data(), value.size()};
}

// What luau-analyze runs with: every flag of Luau's own that Luau doesn't list
// as experimental. Flags are process-wide, so they're set once, before the
// first frontend reads them.
void setFlags()
{
    static const bool isSet = []
    {
        for (Luau::FValue<bool>* flag = Luau::FValue<bool>::list; flag; flag = flag->next)
            if (std::strncmp(flag->name, "Luau", 4) == 0 && !Luau::isAnalysisFlagExperimental(flag->name))
                flag->value = true;
        return true;
    }();
    (void)isSet;
}

struct FileResolver final : Luau::FileResolver
{
    explicit FileResolver(LuckHost host)
        : host(host)
    {
    }

    std::optional<Luau::SourceCode> readSource(const Luau::ModuleName& name) override
    {
        Luau::SourceCode code{{}, Luau::SourceCode::Module};
        if (!host.source(host.context, str(name), &code.source))
            return std::nullopt;
        return code;
    }

    std::optional<Luau::ModuleInfo> resolveModule(const Luau::ModuleInfo* context, Luau::AstExpr* expr, const Luau::TypeCheckLimits&) override
    {
        const Luau::AstExprConstantString* path = expr->as<Luau::AstExprConstantString>();
        if (!context || !path)
            return std::nullopt;

        Luau::ModuleInfo info;
        if (!host.resolve(host.context, str(context->name), {path->value.data, path->value.size}, &info.name))
            return std::nullopt;
        return info;
    }

    LuckHost host;
};

// Analysis copies what it needs out of a config before it asks for the next
// one, so one config serves every module in turn.
struct ConfigResolver final : Luau::ConfigResolver
{
    explicit ConfigResolver(LuckHost host)
        : host(host)
    {
    }

    const Luau::Config& getConfig(const Luau::ModuleName& name, const Luau::TypeCheckLimits&) const override
    {
        config = Luau::Config{};
        host.config(host.context, str(name), &config);
        return config;
    }

    LuckHost host;
    mutable Luau::Config config;
};

struct Checker
{
    explicit Checker(LuckHost host)
        : fileResolver(host)
        , configResolver(host)
        , frontend(Luau::SolverMode::New, &fileResolver, &configResolver)
    {
        Luau::registerBuiltinGlobals(frontend, frontend.globals);
        Luau::freeze(frontend.globals.globalTypes);
    }

    FileResolver fileResolver;
    ConfigResolver configResolver;
    Luau::Frontend frontend;
};

void push(
    LuckErrors errors,
    size_t module,
    bool isSyntaxError,
    const std::string& moduleName,
    const std::optional<Luau::Location>& location,
    const std::string& message
)
{
    LuckError error{module, isSyntaxError, location.has_value(), 0, 0, 0, 0, str(moduleName), str(message)};
    if (location)
    {
        error.begin_line = location->begin.line;
        error.begin_column = location->begin.column;
        error.end_line = location->end.line;
        error.end_column = location->end.column;
    }
    errors.push(errors.context, &error);
}

// Gives a call whose first argument is a string literal the global extern
// type that string names, when it extends the extern type the function
// returns, or else reports the name as an unknown type, as Roblox Studio
// types `Instance.new`. It runs on the worker threads, so it only reads the
// global scope and types, which checks never change.
struct NamedTypeMagic final : Luau::MagicFunction
{
    NamedTypeMagic(const Luau::Scope* scope, const Luau::ExternType* returned)
        : scope(scope)
        , returned(returned)
    {
    }

    std::optional<Luau::WithPredicate<Luau::TypePackId>> handleOldSolver(
        Luau::TypeChecker&,
        const Luau::ScopePtr&,
        const Luau::AstExprCall&,
        Luau::WithPredicate<Luau::TypePackId>
    ) override
    {
        return std::nullopt;
    }

    bool infer(const Luau::MagicFunctionCallContext& context) override
    {
        if (context.callSite->args.size == 0)
            return false;
        Luau::AstExpr* argument = context.callSite->args.data[0];
        const Luau::AstExprConstantString* literal = argument->as<Luau::AstExprConstantString>();
        if (!literal)
            return false;
        std::string name(literal->value.data, literal->value.size);
        auto named = scope->exportedTypeBindings.find(name);
        const Luau::ExternType* type =
            named == scope->exportedTypeBindings.end() ? nullptr : Luau::get<Luau::ExternType>(Luau::follow(named->second.type));
        if (!type || !Luau::isSubclass(type, returned))
        {
            Luau::UnknownSymbol unknown{std::move(name), Luau::UnknownSymbol::Type};
            if (FFlag::LuauCyclicRequireTypeInference)
                context.solver->reportError(std::move(unknown), argument->location, *context.constraint->moduleName);
            else
                context.solver->DEPRECATED_reportError(std::move(unknown), argument->location);
            return false;
        }
        Luau::asMutable(context.result)->ty.emplace<Luau::BoundTypePack>(context.solver->arena->addTypePack({named->second.type}));
        return true;
    }

    const Luau::Scope* scope;
    const Luau::ExternType* returned;
};

void pushTypeError(Checker& checker, LuckErrors errors, size_t module, const Luau::TypeError& error)
{
    bool isSyntaxError = Luau::get_if<Luau::SyntaxError>(&error.data) != nullptr;
    std::string message = Luau::toString(error, Luau::TypeErrorToStringOptions{&checker.fileResolver});
    push(errors, module, isSyntaxError, error.moduleName, error.location, message);
}

LuckLocation location(const Luau::Location& from)
{
    return {from.begin.line, from.begin.column, from.end.line, from.end.column};
}

// Types as Luau prints them, with the names of a function's parameters.
Luau::ToStringOptions typeOptions()
{
    Luau::ToStringOptions options;
    options.functionTypeArguments = true;
    return options;
}

// What Analysis records of the declaration a name refers to, which outlives
// the LuckDocs it lends.
struct Docs
{
    std::optional<std::string> symbol;
    std::optional<std::string> module;
    Luau::Location declaration;

    LuckDocs raw() const
    {
        LuckDocs docs{symbol.has_value(), {}, module.has_value(), {}, location(declaration)};
        if (symbol)
            docs.symbol = str(*symbol);
        if (module)
            docs.module = str(*module);
        return docs;
    }

    void addProperty(const Luau::Property& property, Luau::TypeId declaring)
    {
        if (!symbol && property.documentationSymbol)
            symbol = property.documentationSymbol;
        std::optional<Luau::Location> at = property.location ? property.location : property.typeLocation;
        std::optional<Luau::ModuleName> name = Luau::getDefinitionModuleName(declaring);
        if (at && name)
        {
            module = std::move(name);
            declaration = *at;
        }
    }
};

// The property `name` of `owner`, from a table, an extern type or a type it
// extends, a part of an intersection, or a string's metatable, with the type
// that declares it.
const Luau::Property* findProperty(Luau::TypeId owner, const std::string& name, Luau::TypeId& declaring)
{
    owner = Luau::follow(owner);
    if (const Luau::TableType* table = Luau::get<Luau::TableType>(owner))
    {
        auto property = table->props.find(name);
        if (property == table->props.end())
            return nullptr;
        declaring = owner;
        return &property->second;
    }
    if (Luau::get<Luau::ExternType>(owner))
    {
        for (std::optional<Luau::TypeId> current = owner; current;)
        {
            const Luau::ExternType* externType = Luau::get<Luau::ExternType>(Luau::follow(*current));
            if (!externType)
                return nullptr;
            if (auto property = externType->props.find(name); property != externType->props.end())
            {
                declaring = Luau::follow(*current);
                return &property->second;
            }
            current = externType->parent;
        }
        return nullptr;
    }
    if (const Luau::IntersectionType* intersection = Luau::get<Luau::IntersectionType>(owner))
    {
        for (Luau::TypeId part : intersection->parts)
            if (const Luau::Property* property = findProperty(part, name, declaring))
                return property;
        return nullptr;
    }
    const Luau::PrimitiveType* primitive = Luau::get<Luau::PrimitiveType>(owner);
    const Luau::TableType* metatable =
        primitive && primitive->metatable ? Luau::get<Luau::TableType>(Luau::follow(*primitive->metatable)) : nullptr;
    if (!metatable)
        return nullptr;
    auto index = metatable->props.find("__index");
    if (index == metatable->props.end() || !index->second.readTy)
        return nullptr;
    return findProperty(*index->second.readTy, name, declaring);
}

// Each overload of `type` as Luau prints a function named "", with where in
// that text each parameter is, leaving out `self` for a method call.
void pushSignatures(Luau::TypeId type, bool isMethodCall, LuckDescribed out)
{
    type = Luau::follow(type);
    std::vector<Luau::TypeId> overloads{type};
    if (const Luau::IntersectionType* intersection = Luau::get<Luau::IntersectionType>(type))
        overloads = intersection->parts;
    for (Luau::TypeId overload : overloads)
    {
        const Luau::FunctionType* function = Luau::get<Luau::FunctionType>(Luau::follow(overload));
        if (!function)
            continue;
        Luau::ToStringOptions options = typeOptions();
        options.hideFunctionSelfArgument = isMethodCall;
        std::string label = Luau::toStringNamedFunction("", *function, options);

        // toStringNamedFunction prints each parameter in turn as these do.
        std::vector<std::string> texts;
        Luau::TypePackIterator param = Luau::begin(function->argTypes);
        for (size_t index = 0; param != Luau::end(function->argTypes); ++param, ++index)
        {
            if (index == 0 && function->hasSelf && isMethodCall)
                continue;
            bool isNamed = index < function->argNames.size() && function->argNames[index];
            texts.push_back((isNamed ? function->argNames[index]->name : "_") + ": " + Luau::toString(*param, options));
        }
        if (std::optional<Luau::TypePackId> tail = param.tail())
        {
            const Luau::VariadicTypePack* variadic = Luau::get<Luau::VariadicTypePack>(Luau::follow(*tail));
            if (!variadic || !variadic->hidden)
                texts.push_back("...: " + (variadic ? Luau::toString(variadic->ty, options) : Luau::toString(*tail, options)));
        }
        std::vector<uint32_t> params;
        size_t from = label.find('(');
        for (const std::string& text : texts)
        {
            from = from == std::string::npos ? from : label.find(text, from);
            if (from == std::string::npos)
                break;
            params.push_back(uint32_t(from));
            params.push_back(uint32_t(from + text.size()));
            from += text.size();
        }
        out.signature(out.context, str(label), params.data(), params.size() / 2);
    }
}

} // namespace

extern "C"
{

void luck_string_assign(void* string, LuckStr value) noexcept
{
    static_cast<std::string*>(string)->assign(value.data, value.len);
}

// As luau-analyze reads one: without alias options, an alias would stop the
// parse before the keys after it.
void luck_config_apply_luaurc(void* config, LuckStr text, LuckErrors errors) noexcept
{
    Luau::ConfigOptions options;
    options.aliasOptions = Luau::ConfigOptions::AliasOptions{std::nullopt, true};
    if (std::optional<std::string> error = Luau::parseConfig(std::string(text.data, text.len), *static_cast<Luau::Config*>(config), options))
        push(errors, 0, false, std::string(), std::nullopt, *error);
}

void* luck_checker_new(LuckHost host) noexcept
{
    setFlags();
    return new Checker(host);
}

void luck_checker_free(void* checker) noexcept
{
    delete static_cast<Checker*>(checker);
}

void luck_checker_load_definitions(void* raw, LuckStr name, LuckStr source, LuckErrors errors) noexcept
{
    Checker& checker = *static_cast<Checker*>(raw);
    Luau::Frontend& frontend = checker.frontend;
    std::string packageName(name.data, name.len);

    Luau::unfreeze(frontend.globals.globalTypes);
    try
    {
        Luau::LoadDefinitionFileResult result =
            frontend.loadDefinitionFile(frontend.globals, frontend.globals.globalScope, {source.data, source.len}, packageName, false);
        for (const Luau::ParseError& error : result.parseResult.errors)
            push(errors, 0, true, packageName, error.getLocation(), error.getMessage());
        if (result.module)
            for (const Luau::TypeError& error : result.module->errors)
                pushTypeError(checker, errors, 0, error);
    }
    catch (const Luau::InternalCompilerError& error)
    {
        push(errors, 0, false, packageName, error.location, error.message);
    }
    Luau::freeze(frontend.globals.globalTypes);
}

void luck_checker_return_named_type(void* raw, LuckStr owner, LuckStr member) noexcept
{
    Luau::GlobalTypes& globals = static_cast<Checker*>(raw)->frontend.globals;
    std::string ownerName(owner.data, owner.len);
    Luau::TableType::Props* props = nullptr;
    if (std::optional<Luau::Binding> global = Luau::tryGetGlobalBinding(globals, ownerName))
    {
        if (Luau::TableType* table = Luau::getMutable<Luau::TableType>(Luau::follow(global->typeId)))
            props = &table->props;
    }
    else if (auto type = globals.globalScope->exportedTypeBindings.find(ownerName); type != globals.globalScope->exportedTypeBindings.end())
    {
        if (Luau::ExternType* externType = Luau::getMutable<Luau::ExternType>(Luau::follow(type->second.type)))
            props = &externType->props;
    }
    if (!props)
        return;
    auto prop = props->find(std::string(member.data, member.len));
    if (prop == props->end() || !prop->second.readTy)
        return;
    Luau::TypeId function = Luau::follow(*prop->second.readTy);
    const Luau::FunctionType* signature = Luau::get<Luau::FunctionType>(function);
    if (!signature)
        return;
    std::optional<Luau::TypeId> returnType = Luau::first(signature->retTypes);
    const Luau::ExternType* returned = returnType ? Luau::get<Luau::ExternType>(Luau::follow(*returnType)) : nullptr;
    if (!returned)
        return;

    Luau::unfreeze(globals.globalTypes);
    Luau::attachMagicFunction(function, std::make_shared<NamedTypeMagic>(globals.globalScope.get(), returned));
    Luau::freeze(globals.globalTypes);
}

void luck_checker_bind_type_namespace(void* raw, LuckStr space, LuckStr supertype) noexcept
{
    Luau::GlobalTypes& globals = static_cast<Checker*>(raw)->frontend.globals;
    Luau::Scope& scope = *globals.globalScope;
    std::string prefix(space.data, space.len);
    auto super = scope.exportedTypeBindings.find(std::string(supertype.data, supertype.len));
    if (super == scope.exportedTypeBindings.end())
        return;
    Luau::TypeId superType = Luau::follow(super->second.type);

    std::unordered_map<Luau::Name, Luau::TypeFun>& bound = scope.importedTypeBindings[prefix];
    Luau::unfreeze(globals.globalTypes);
    for (auto it = scope.exportedTypeBindings.begin(); it != scope.exportedTypeBindings.end();)
    {
        Luau::ExternType* type = Luau::getMutable<Luau::ExternType>(Luau::follow(it->second.type));
        if (type && type->parent && Luau::follow(*type->parent) == superType && it->first.size() > prefix.size() &&
            it->first.compare(0, prefix.size(), prefix) == 0)
        {
            std::string rest = it->first.substr(prefix.size());
            type->name = prefix + "." + rest;
            bound.emplace(std::move(rest), it->second);
            it = scope.exportedTypeBindings.erase(it);
        }
        else
            ++it;
    }
    Luau::freeze(globals.globalTypes);
}

void luck_checker_mark_dirty(void* checker, LuckStr module) noexcept
{
    static_cast<Checker*>(checker)->frontend.markDirty(std::string(module.data, module.len));
}

void luck_checker_retain_type_graphs(void* checker) noexcept
{
    static_cast<Checker*>(checker)->frontend.options.retainFullTypeGraphs = true;
}

// Describes the type reference, local, or expression at a position of a
// module the checker kept the types of, and each signature of its type.
bool luck_checker_describe(void* raw, LuckStr moduleName, uint32_t line, uint32_t column, LuckDescribed out) noexcept
{
    Luau::Frontend& frontend = static_cast<Checker*>(raw)->frontend;
    std::string name(moduleName.data, moduleName.len);
    const Luau::SourceModule* source = frontend.getSourceModule(name);
    Luau::ModulePtr module = frontend.moduleResolver.getModule(name);
    if (!source || !module)
        return false;
    Luau::Position position{line, column};

    try
    {
        std::optional<Luau::TypeId> type;
        Luau::Location at;
        Docs docs;
        bool isMethodCall = false;
        std::vector<Luau::AstNode*> ancestry = Luau::findAstAncestryOfPosition(*source, position, /* includeTypes */ true);
        if (!ancestry.empty() && ancestry.back()->asType())
        {
            const Luau::AstTypeReference* reference = ancestry.back()->as<Luau::AstTypeReference>();
            Luau::ScopePtr scope = Luau::findScopeAtPosition(*module, position);
            if (!reference || !scope)
                return false;
            std::optional<Luau::TypeFun> named = reference->prefix ? scope->lookupImportedType(reference->prefix->value, reference->name.value)
                                                                   : scope->lookupType(reference->name.value);
            if (!named)
                return false;
            type = named->type;
            at = reference->location;
            docs.symbol = Luau::follow(named->type)->documentationSymbol;
        }
        else
        {
            Luau::ExprOrLocal found = Luau::findExprOrLocalAtPosition(*source, position);
            if (Luau::AstExpr* expr = found.getExpr())
            {
                if (const Luau::TypeId* exprType = module->astTypes.find(expr))
                    type = *exprType;
                const Luau::AstExprIndexName* index = expr->as<Luau::AstExprIndexName>();
                const Luau::TypeId* owner = index ? module->astTypes.find(index->expr) : nullptr;
                Luau::TypeId declaring = nullptr;
                if (const Luau::Property* property = owner ? findProperty(*owner, index->index.value, declaring) : nullptr)
                    docs.addProperty(*property, declaring);
                isMethodCall = index && index->op == ':';
            }
            else if (Luau::AstLocal* local = found.getLocal())
            {
                if (Luau::ScopePtr scope = Luau::findScopeAtPosition(*module, position))
                    type = scope->lookup(local);
            }
            if (!type || !found.getLocation())
                return false;
            at = *found.getLocation();
            if (!docs.symbol)
                docs.symbol = Luau::getDocumentationSymbolAtPosition(*source, *module, position);
        }

        Luau::ToStringOptions options = typeOptions();
        std::string text = Luau::toString(*type, options);
        LuckDocs rawDocs = docs.raw();
        out.describe(out.context, location(at), str(text), &rawDocs);
        pushSignatures(*type, isMethodCall, out);
        return true;
    }
    catch (const Luau::InternalCompilerError&)
    {
        return false;
    }
}

// Luau's autocomplete at a position of a module the checker kept the types
// of, leaving out what the index there can't reach, such as a property after
// a colon.
void luck_checker_autocomplete(void* raw, LuckStr moduleName, uint32_t line, uint32_t column, LuckEntries out) noexcept
{
    Luau::Frontend& frontend = static_cast<Checker*>(raw)->frontend;
    std::string name(moduleName.data, moduleName.len);
    Luau::ModulePtr module = frontend.moduleResolver.getModule(name);
    if (!module)
        return;

    try
    {
        Luau::AutocompleteResult result = Luau::autocomplete(
            frontend,
            name,
            Luau::Position{line, column},
            [](std::string, std::optional<const Luau::ExternType*>, std::optional<std::string>)
            {
                return std::optional<Luau::AutocompleteEntryMap>{};
            }
        );
        const Luau::AstExprIndexName* index = result.ancestry.empty() ? nullptr : result.ancestry.back()->as<Luau::AstExprIndexName>();
        const Luau::TypeId* owner = index ? module->astTypes.find(index->expr) : nullptr;
        Luau::ToStringOptions options = typeOptions();
        for (const auto& [entryName, entry] : result.entryMap)
        {
            if (entry.wrongIndexType)
                continue;
            Docs docs;
            docs.symbol = entry.documentationSymbol;
            Luau::TypeId declaring = nullptr;
            if (const Luau::Property* property = owner ? findProperty(*owner, entryName, declaring) : nullptr)
                docs.addProperty(*property, declaring);
            std::string type = entry.type ? Luau::toString(*entry.type, options) : std::string();
            LuckEntry raw{
                str(entryName),
                uint8_t(entry.kind),
                entry.type.has_value(),
                str(type),
                entry.deprecated,
                entry.insertText.has_value(),
                entry.insertText ? str(*entry.insertText) : LuckStr{},
                docs.raw(),
            };
            out.push(out.context, &raw);
        }
    }
    catch (const Luau::InternalCompilerError&)
    {
    }
}

uint8_t luck_checker_check(
    void* raw,
    const LuckStr* modules,
    size_t count,
    const void* cancellation,
    LuckExecutor executor,
    LuckErrors errors
) noexcept
{
    Checker& checker = *static_cast<Checker*>(raw);
    Luau::Frontend& frontend = checker.frontend;

    std::vector<Luau::ModuleName> names;
    names.reserve(count);
    for (size_t i = 0; i < count; ++i)
        names.emplace_back(modules[i].data, modules[i].len);

    Luau::FrontendOptions options = frontend.options;
    options.cancellationToken = *static_cast<const CancellationToken*>(cancellation);

    try
    {
        frontend.queueModuleCheck(names);
        frontend.checkQueuedModules(
            options,
            [executor](std::vector<std::function<void()>> tasks)
            {
                for (std::function<void()>& task : tasks)
                    executor.submit(executor.context, new std::function<void()>(std::move(task)));
            }
        );
        if (options.cancellationToken->requested())
            return LUCK_CANCELLED;

        for (size_t i = 0; i < count; ++i)
            if (std::optional<Luau::CheckResult> result = frontend.getCheckResult(names[i], false))
                for (const Luau::TypeError& error : result->errors)
                    pushTypeError(checker, errors, i, error);
        return LUCK_CHECKED;
    }
    catch (const Luau::UserCancelError&)
    {
        return LUCK_CANCELLED;
    }
    catch (const Luau::InternalCompilerError& error)
    {
        push(errors, count, false, error.moduleName.value_or(std::string()), error.location, error.message);
        return LUCK_INTERNAL_ERROR;
    }
}

void luck_task_run(void* task) noexcept
{
    std::unique_ptr<std::function<void()>> owned(static_cast<std::function<void()>*>(task));
    (*owned)();
}

void* luck_cancellation_new() noexcept
{
    return new CancellationToken(std::make_shared<Luau::FrontendCancellationToken>());
}

void luck_cancellation_cancel(const void* token) noexcept
{
    (*static_cast<const CancellationToken*>(token))->cancel();
}

bool luck_cancellation_is_cancelled(const void* token) noexcept
{
    return (*static_cast<const CancellationToken*>(token))->requested();
}

void luck_cancellation_free(void* token) noexcept
{
    delete static_cast<CancellationToken*>(token);
}

LuckStr luck_builtin_definitions() noexcept
{
    setFlags();
    static const std::string source = Luau::getBuiltinDefinitionSource();
    return str(source);
}

} // extern "C"
