defmodule XbergRerankerBackendBridge do
  @moduledoc """
  GenServer bridge for RerankerBackend implementation in xberg.

  Handles incoming trait method calls from Rust and dispatches them to an implementation module.
  """

  use GenServer

  require Logger

  @doc """
  Start a GenServer linked to the current process.

  impl_module should be a module that implements the RerankerBackend trait methods.
  """
  def start_link(impl_module) do
    GenServer.start_link(__MODULE__, impl_module, name: __MODULE__)
  end

  @impl GenServer
  def init(impl_module) do
    {:ok, impl_module}
  end

  @doc """
  Handle an incoming trait call message.

  Message format: {:trait_call, method_atom, args, reply_id}

  `args` arrives as a native Erlang map (no JSON decode); the reply stays JSON.
  """
  @impl GenServer
  def handle_info({:trait_call, method, args, reply_id}, impl_module) do
    try do
      method_name = to_string(method)
      ordered_args = ordered_args(impl_module, method_name, args)

      # Dispatch to the implementation module
      result = apply(impl_module, String.to_existing_atom(method_name), ordered_args)

      # Send result back to Rust
      Xberg.Native.complete_trait_call(reply_id, Jason.encode!(result))
    rescue
      e ->
        Logger.error("Error calling {impl_module}.{method}: {Exception.message(e)}")
        Xberg.Native.fail_trait_call(reply_id, Exception.message(e))
    end

    {:noreply, impl_module}
  end

  defp ordered_args(impl_module, method_name, args) when is_map(args) do
    if function_exported?(impl_module, :__alef_arg_order__, 1) do
      impl_module.__alef_arg_order__(method_name)
      |> Enum.map(&Map.fetch!(args, &1))
    else
      args
      |> Map.keys()
      |> Enum.sort()
      |> Enum.map(&Map.fetch!(args, &1))
    end
  end

  defp ordered_args(_impl_module, _method_name, args) when is_list(args), do: args

  @doc """
  Register an implementation module, starting a GenServer to handle trait calls.
  """
  def register(impl_module) do
    plugin_name = impl_module.name()
    {:ok, pid} = start_link(impl_module)

    # Names of the functions the implementation module exports. Rust-defaulted
    # trait methods outside this list keep their Rust default behavior instead
    # of being dispatched to the module.
    implemented_methods =
      impl_module.__info__(:functions)
      |> Enum.map(fn {name, _arity} -> Atom.to_string(name) end)
      |> Enum.uniq()

    Xberg.Native.register_reranker_backend(pid, plugin_name, implemented_methods)
  end
end
