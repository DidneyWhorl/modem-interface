/**
 * useATCommand Hook
 * 
 * Mutation for executing AT commands.
 * Results are returned directly, not cached.
 */

import { useMutation } from '@tanstack/react-query';
import { executeATCommand } from '@/api';
import type { ATCommandRequest, ATCommandResponse } from '@/types/api';

export function useATCommand() {
  return useMutation<ATCommandResponse, Error, ATCommandRequest>({
    mutationFn: executeATCommand,
    // AT command results aren't cached - they're point-in-time responses
  });
}
